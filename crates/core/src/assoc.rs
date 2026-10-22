//! Association rules: endpoint <-> process <-> service.
//!
//! Every rule here is driven by an *observable* join key, never by
//! heuristics like "port 80 must be nginx" or "process name contains nginx".
//!
//! Join keys available to a normal user:
//!   1. socket `inode`        (`ss -e` / `/proc/net/*`)   -> `/proc/<pid>/fd`
//!   2. socket `cgroup`       (`ss -e`)                   -> `/system.slice/<unit>.service`
//!   3. socket `uid`          (`ss -e`)                   -> uid -> user name
//!   4. unit `MainPID`        (systemd D-Bus / systemctl  show)
//!   5. socket unit `Triggers`(systemd)                   -> activated service

use crate::model::*;

/// Extract the unit name from a cgroup v2 path, if it names a systemd unit.
///
/// Examples:
///   `/system.slice/ssh.socket`                     -> Some(("ssh.socket", System))
///   `/system.slice/docker-abc...scope`             -> Some(("docker-abc...scope", System))
///   `/user.slice/user-1000.slice/session-13.scope` -> Some(("session-13.scope", User))
///   `/system.slice/system-dbus\x2d:1.2.slice/...`  -> None (slice, not unit of interest)
pub fn unit_from_cgroup(path: &str) -> Option<(String, UnitScope)> {
    let scope = if path.starts_with("/user.slice/") {
        UnitScope::User
    } else if path.starts_with("/system.slice/") || path.starts_with("/init.scope") {
        UnitScope::System
    } else if path.starts_with("/app.slice/") || path.starts_with("/background.slice/") {
        UnitScope::User
    } else {
        return None;
    };

    // Take the last component that looks like a leaf unit.
    let leaf = path.rsplit('/').find(|c| !c.is_empty())?;
    if leaf.ends_with(".service") || leaf.ends_with(".socket") || leaf.ends_with(".scope") {
        Some((leaf.to_string(), scope))
    } else {
        None
    }
}

/// Find a socket unit that listens on the same address/port as `ep`.
///
/// Used for *user-level* socket activation, where `ss -e` reports the cgroup
/// as the user manager's `init.scope` rather than the socket unit (verified
/// experimentally; see SPIKE-REPORT.md).
///
/// Matching on the unit's own `Listen=` property is real evidence: it is the
/// configuration systemd actually bound. We require the unit to be active and
/// we never match on a bare port when protocol or address family disagree.
pub fn socket_unit_for_endpoint<'a>(
    ep: &Endpoint,
    services: &'a [ServiceInfo],
) -> Option<&'a ServiceInfo> {
    services
        .iter()
        .filter(|s| s.kind == UnitKind::Socket && s.active_state == ActiveState::Active)
        .find(|s| {
            s.listen.iter().any(|l| {
                let Some((addr, port)) = parse_listen(l) else {
                    return false;
                };
                port == ep.local_port
                    && matches_protocol(l, ep.protocol)
                    && addr_matches(&addr, &ep.local_addr)
            })
        })
}

/// Parse a `Listen=` value like `0.0.0.0:22 (Stream)`.
/// Returns address and port for inet sockets only (unix sockets -> None).
fn parse_listen(l: &str) -> Option<(String, u16)> {
    let (sock, kind) = match l.rsplit_once(' ') {
        Some((s, k)) => (s, k.trim_matches(|c| c == '(' || c == ')')),
        None => (l, "Stream"),
    };
    if kind != "Stream" && kind != "Datagram" {
        return None;
    }
    if sock.starts_with('/') || sock.starts_with('@') {
        return None;
    }
    let idx = sock.rfind(':')?;
    let (addr, port) = sock.split_at(idx);
    let port: u16 = port[1..].trim_end_matches(']').parse().ok()?;
    // `[::]:22` -> addr part is `[::]`; strip both brackets together.
    let addr = addr
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string();
    Some((addr, port))
}

/// Stream => tcp, Datagram => udp. The type token is part of the value.
fn matches_protocol(listen_value: &str, proto: Protocol) -> bool {
    if listen_value.contains("(Datagram)") {
        proto == Protocol::Udp
    } else {
        proto == Protocol::Tcp
    }
}

/// A wildcard listen (`0.0.0.0`, `::`) covers any local address of its family.
fn addr_matches(listen_addr: &str, ep_addr: &str) -> bool {
    if listen_addr == ep_addr {
        return true;
    }
    let is_v6 = |a: &str| a.contains(':');
    if is_v6(listen_addr) != is_v6(ep_addr) {
        return false;
    }
    matches!(listen_addr, "0.0.0.0" | "::" | "*")
}

/// The unit that *owns the socket fd* for socket-activated endpoints.
///
/// For `/system.slice/ssh.socket` this yields `ssh.socket`. That is not a
/// service, so callers must resolve it via socket activation.
pub fn socket_owner_unit(ep: &Endpoint) -> Option<(String, UnitScope)> {
    ep.cgroup.as_deref().and_then(unit_from_cgroup)
}

/// Rank/deduplicate evidence, strongest first. Pure function; tested.
pub fn rank_evidence(mut ev: Vec<Evidence>) -> Vec<Evidence> {
    ev.sort_by(|a, b| b.kind.strength().cmp(&a.kind.strength()));
    ev.dedup_by(|a, b| a.kind == b.kind && a.detail == b.detail);
    ev
}

/// Build endpoint -> service associations from cgroup paths alone.
///
/// This works with zero process privileges: `ss -e` exposes cgroup for
/// sockets owned by other users. A `.socket` cgroup is resolved to the
/// service it triggers using `Triggers=`, and the association is marked as
/// `SocketActivation` evidence (not `CgroupUnitMatch`) so the UI can explain
/// that the *listening socket* belongs to the unit, not the running process.
pub fn associate_endpoints_to_units(
    endpoints: &[Endpoint],
    services: &[ServiceInfo],
) -> (Vec<Association>, Vec<CollectionError>) {
    let mut out = Vec::new();
    let mut errors = Vec::new();

    for ep in endpoints {
        // Case U: user-level socket activation. `ss -e` reports the fd as
        // owned by `systemd --user` under `init.scope`, so the cgroup names no
        // unit. Recover the link from the socket unit's own `Listen=` value.
        if let Some(cg) = ep.cgroup.as_deref() {
            if cg.ends_with("/init.scope") {
                if let Some(su) = socket_unit_for_endpoint(ep, services) {
                    let mut ev = vec![Evidence {
                        kind: EvidenceKind::CgroupUnitMatch,
                        detail: format!("ss cgroup {} (owned by systemd --user)", cg),
                    }];
                    let target = su
                        .triggers
                        .first()
                        .cloned()
                        .unwrap_or_else(|| su.id.clone());
                    if !su.triggers.is_empty() {
                        ev.push(Evidence {
                            kind: EvidenceKind::SocketActivation,
                            detail: format!("{} Triggers={} Listen={:?}", su.id, target, su.listen),
                        });
                    }
                    out.push(Association::new(
                        AssociationKind::EndpointToService {
                            endpoint: ep.key(),
                            unit: target,
                            scope: su.scope,
                        },
                        rank_evidence(ev),
                    ));
                    continue;
                }
            }
        }

        let Some((unit, scope)) = socket_owner_unit(ep) else {
            continue;
        };
        let key = ep.key();
        let unit_present = services.iter().any(|s| s.id == unit && s.scope == scope);

        if unit.ends_with(".socket") {
            // Resolve via socket activation.
            let socket_unit = services.iter().find(|s| s.id == unit && s.scope == scope);
            match socket_unit {
                Some(su) if !su.triggers.is_empty() => {
                    for svc in &su.triggers {
                        let mut ev = vec![Evidence {
                            kind: EvidenceKind::SocketActivation,
                            detail: format!("{} Triggers={}", unit, svc),
                        }];
                        if let Some(cg) = &ep.cgroup {
                            ev.push(Evidence {
                                kind: EvidenceKind::CgroupUnitMatch,
                                detail: format!("cgroup:{}", cg),
                            });
                        }
                        out.push(Association::new(
                            AssociationKind::EndpointToService {
                                endpoint: key.clone(),
                                unit: svc.clone(),
                                scope,
                            },
                            rank_evidence(ev),
                        ));
                    }
                }
                Some(_) => {
                    // Socket unit exists but triggers nothing we can see.
                    let mut ev = vec![];
                    if let Some(cg) = &ep.cgroup {
                        ev.push(Evidence {
                            kind: EvidenceKind::CgroupUnitMatch,
                            detail: format!("cgroup:{}", cg),
                        });
                    }
                    out.push(Association::new(
                        AssociationKind::EndpointToService {
                            endpoint: key.clone(),
                            unit: unit.clone(),
                            scope,
                        },
                        rank_evidence(ev),
                    ));
                }
                None if unit_present => {}
                None => {
                    errors.push(CollectionError::UnitNotFound { unit: unit.clone() });
                }
            }
        } else if unit.ends_with(".scope") && (unit == "init.scope" || unit.starts_with("session-"))
        {
            // These scopes describe WHERE a process runs, not WHICH unit owns
            // the port. Emitting them as EndpointToService would mislead the
            // UI into showing "session-13.scope" as the owning service.
            continue;
        } else if unit.ends_with(".service") || unit.ends_with(".scope") {
            let mut ev = vec![];
            if let Some(cg) = &ep.cgroup {
                ev.push(Evidence {
                    kind: EvidenceKind::CgroupUnitMatch,
                    detail: format!("cgroup:{}", cg),
                });
            }
            out.push(Association::new(
                AssociationKind::EndpointToService {
                    endpoint: key,
                    unit: unit.clone(),
                    scope,
                },
                rank_evidence(ev),
            ));
            // The cgroup itself is proof the unit name is real, but if our
            // unit listing did not include it we must say so: the UI should
            // not present an unverified unit as a known service.
            if !unit_present && !unit.ends_with(".scope") {
                errors.push(CollectionError::UnitNotFound { unit });
            }
        }
    }

    (out, errors)
}

/// Build endpoint -> process associations.
///
/// Inputs are the raw observations; we never look at process *names*.
///   - `ss -p` pid (only for our own processes)
///   - pid -> cgroup that equals the socket's cgroup
///   - pid -> fd contains socket inode
///
/// `pid_cgroups` is supplied by the collector because a process's cgroup is
/// read from `/proc/<pid>/cgroup`, not from the `ProcessInfo` struct.
pub fn associate_endpoints_to_processes(
    endpoints: &[Endpoint],
    processes: &[ProcessInfo],
    ss_pid_hints: &std::collections::HashMap<EndpointKey, u32>,
    inode_hits: &std::collections::HashMap<u64, Vec<u32>>,
    pid_cgroups: &std::collections::HashMap<u32, String>,
) -> Vec<Association> {
    let mut out = Vec::new();

    for ep in endpoints {
        let key = ep.key();
        let mut ev: Vec<Evidence> = Vec::new();
        let mut identities: Vec<ProcessIdentity> = Vec::new();

        // (a) explicit pid from ss -p
        if let Some(pid) = ss_pid_hints.get(&key) {
            if let Some(p) = processes.iter().find(|p| p.pid == *pid) {
                if let Some(ticks) = p.start_time_ticks {
                    identities.push(ProcessIdentity {
                        pid: *pid,
                        start_time_ticks: ticks,
                    });
                    ev.push(Evidence {
                        kind: EvidenceKind::SocketProcessField,
                        detail: format!("ss -p users:((pid={}))", pid),
                    });
                }
            }
        }

        // (b) socket inode found in /proc/<pid>/fd
        if let Some(inode) = ep.inode {
            if let Some(pids) = inode_hits.get(&inode) {
                for pid in pids {
                    if let Some(p) = processes.iter().find(|p| p.pid == *pid) {
                        if let Some(ticks) = p.start_time_ticks {
                            identities.push(ProcessIdentity {
                                pid: *pid,
                                start_time_ticks: ticks,
                            });
                            ev.push(Evidence {
                                kind: EvidenceKind::SocketInodeInProcFd,
                                detail: format!("inode {} in /proc/{}/fd", inode, pid),
                            });
                        }
                    }
                }
            }
        }

        // (c) cgroup match between socket and process.
        //
        // This runs even when (a)/(b) already produced an identity: the cgroup
        // is an independent join, and restricting it to the "no identity yet"
        // case meant an endpoint whose `ss -p` hint pointed at a process we
        // could not inspect ended up with NO association at all, even though
        // its cgroup unambiguously named an owner. That accounted for most of
        // the spurious "permission denied" endpoints.
        if let Some(scg) = ep.cgroup.as_deref() {
            for p in processes {
                if let Some(pcg) = pid_cgroups.get(&p.pid) {
                    if pcg == scg && !identities.iter().any(|i| i.pid == p.pid) {
                        if let Some(ticks) = p.start_time_ticks {
                            identities.push(ProcessIdentity {
                                pid: p.pid,
                                start_time_ticks: ticks,
                            });
                            ev.push(Evidence {
                                kind: EvidenceKind::CgroupUnitMatch,
                                detail: format!("socket cgroup {} == pid {} cgroup", scg, p.pid),
                            });
                        }
                    }
                }
            }
        }

        identities.sort_by_key(|i| i.pid);
        identities.dedup();
        let ev = rank_evidence(ev);
        for id in identities {
            out.push(Association::new(
                AssociationKind::EndpointToProcess {
                    endpoint: key.clone(),
                    identity: id,
                },
                ev.clone(),
            ));
        }
    }

    out
}

/// Build service -> process associations from `MainPID`.
pub fn associate_services_to_processes(
    services: &[ServiceInfo],
    processes: &[ProcessInfo],
) -> Vec<Association> {
    let mut out = Vec::new();
    for svc in services {
        let Some(pid) = svc.main_pid else { continue };
        if pid == 0 {
            continue;
        }
        if let Some(p) = processes.iter().find(|p| p.pid == pid) {
            if let Some(ticks) = p.start_time_ticks {
                out.push(Association::new(
                    AssociationKind::ServiceToProcess {
                        unit: svc.id.clone(),
                        identity: ProcessIdentity {
                            pid,
                            start_time_ticks: ticks,
                        },
                    },
                    vec![Evidence {
                        kind: EvidenceKind::MainPidMatch,
                        detail: format!("MainPID={}", pid),
                    }],
                ));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ep(
        proto: Protocol,
        addr: &str,
        port: u16,
        cgroup: Option<&str>,
        inode: Option<u64>,
    ) -> Endpoint {
        Endpoint {
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
            inode,
            uid: Some(1000),
            cgroup: cgroup.map(|s| s.to_string()),
        }
    }

    #[test]
    fn cgroup_to_unit_system() {
        assert_eq!(
            unit_from_cgroup("/system.slice/ssh.socket"),
            Some(("ssh.socket".to_string(), UnitScope::System))
        );
        assert_eq!(
            unit_from_cgroup("/system.slice/nginx.service"),
            Some(("nginx.service".to_string(), UnitScope::System))
        );
    }

    #[test]
    fn cgroup_to_unit_user_slice_gives_leaf_scope_not_unit() {
        // A session scope is not a service; we still surface it as a unit
        // because it is a real cgroup we can name.
        assert_eq!(
            unit_from_cgroup("/user.slice/user-1000.slice/session-13.scope"),
            Some(("session-13.scope".to_string(), UnitScope::User))
        );
    }

    #[test]
    fn cgroup_slice_without_unit_returns_none() {
        assert_eq!(unit_from_cgroup("/system.slice"), None);
        assert_eq!(unit_from_cgroup("/"), None);
        assert_eq!(unit_from_cgroup(""), None);
    }

    #[test]
    fn docker_scope_is_preserved_verbatim() {
        let p = "/system.slice/docker-3c1695f9cbc0.scope";
        let (leaf, scope) = unit_from_cgroup(p).unwrap();
        assert_eq!(leaf, "docker-3c1695f9cbc0.scope");
        assert_eq!(scope, UnitScope::System);
    }

    #[test]
    fn socket_unit_resolves_to_triggered_service() {
        let endpoints = vec![ep(
            Protocol::Tcp,
            "0.0.0.0",
            22,
            Some("/system.slice/ssh.socket"),
            Some(1),
        )];
        let services = vec![ServiceInfo {
            id: "ssh.socket".into(),
            scope: UnitScope::System,
            kind: UnitKind::Socket,
            active_state: ActiveState::Active,
            sub_state: "running".into(),
            load_state: "loaded".into(),
            main_pid: None,
            control_group: Some("/system.slice/ssh.socket".into()),
            listen: vec!["0.0.0.0:22 (Stream)".into()],
            triggers: vec!["ssh.service".into()],
            triggered_by: vec![],
            fragment_path: None,
            source: ServiceSource::Systemctl,
        }];
        let (assoc, errs) = associate_endpoints_to_units(&endpoints, &services);
        assert!(errs.is_empty());
        assert_eq!(assoc.len(), 1);
        match &assoc[0].kind {
            AssociationKind::EndpointToService { unit, .. } => assert_eq!(unit, "ssh.service"),
            _ => panic!("wrong kind"),
        }
        assert!(
            assoc[0]
                .evidence
                .iter()
                .any(|e| e.kind == EvidenceKind::SocketActivation)
        );
        assert!(assoc[0].confident);
    }

    #[test]
    fn service_cgroup_yields_unit_association_without_pid() {
        let endpoints = vec![ep(
            Protocol::Tcp,
            "127.0.0.1",
            9090,
            Some("/system.slice/sing-box.service"),
            None,
        )];
        let services: Vec<ServiceInfo> = vec![];
        let (assoc, errs) = associate_endpoints_to_units(&endpoints, &services);
        // Unit isn't in our service list, so we must report that rather than
        // inventing a display name.
        assert_eq!(assoc.len(), 1);
        assert!(!errs.is_empty());
        assert_eq!(errs[0].code(), "unit_not_found");
    }

    #[test]
    fn no_cgroup_means_no_unit_association_never_port_heuristic() {
        // Port 22 with no cgroup must NOT be linked to ssh.service by port.
        let endpoints = vec![ep(Protocol::Tcp, "0.0.0.0", 22, None, None)];
        let services = vec![ServiceInfo {
            id: "ssh.service".into(),
            scope: UnitScope::System,
            kind: UnitKind::Service,
            active_state: ActiveState::Active,
            sub_state: "running".into(),
            load_state: "loaded".into(),
            main_pid: Some(1),
            control_group: None,
            listen: vec![],
            triggers: vec![],
            triggered_by: vec![],
            fragment_path: None,
            source: ServiceSource::Systemctl,
        }];
        let (assoc, _) = associate_endpoints_to_units(&endpoints, &services);
        assert!(assoc.is_empty(), "must not associate by port number alone");
    }

    #[test]
    fn same_port_different_protocol_are_distinct_keys() {
        let t = ep(Protocol::Tcp, "127.0.0.1", 5000, None, None);
        let u = ep(Protocol::Udp, "127.0.0.1", 5000, None, None);
        assert_ne!(t.key(), u.key());
    }

    #[test]
    fn evidence_ranking_puts_strongest_first_and_dedups() {
        let ev = rank_evidence(vec![
            Evidence {
                kind: EvidenceKind::CgroupUnitMatch,
                detail: "a".into(),
            },
            Evidence {
                kind: EvidenceKind::SocketInodeInProcFd,
                detail: "b".into(),
            },
            Evidence {
                kind: EvidenceKind::CgroupUnitMatch,
                detail: "a".into(),
            },
        ]);
        assert_eq!(ev.len(), 2);
        assert_eq!(ev[0].kind, EvidenceKind::SocketInodeInProcFd);
    }

    #[test]
    fn pid_reuse_is_detectable_via_start_time() {
        let id = ProcessIdentity {
            pid: 42,
            start_time_ticks: 100,
        };
        let mut snap = Snapshot::default();
        snap.processes.push(ProcessInfo {
            pid: 42,
            name: "a".into(),
            cmdline: None,
            uid: None,
            user: None,
            ppid: None,
            start_time_ticks: Some(200), // different process, same pid
            source: PidSource::CgroupProcs,
        });
        assert!(snap.process_by_identity(&id).is_none());
    }

    #[test]
    fn parse_listen_inet_variants() {
        assert_eq!(
            parse_listen("0.0.0.0:22 (Stream)"),
            Some(("0.0.0.0".into(), 22))
        );
        assert_eq!(parse_listen("[::]:22 (Stream)"), Some(("::".into(), 22)));
        assert_eq!(
            parse_listen("127.0.0.1:53 (Datagram)"),
            Some(("127.0.0.1".into(), 53))
        );
        // unix sockets must not be treated as inet
        assert_eq!(parse_listen("/run/cups/cups.sock (Stream)"), None);
        assert_eq!(parse_listen("@abstract (Stream)"), None);
    }

    #[test]
    fn wildcard_matching_respects_family() {
        assert!(addr_matches("0.0.0.0", "127.0.0.1"));
        assert!(addr_matches("::", "::1"));
        assert!(
            !addr_matches("0.0.0.0", "::1"),
            "v4 wildcard must not match v6"
        );
        assert!(!addr_matches("127.0.0.1", "127.0.0.2"));
    }

    #[test]
    fn protocol_matching_from_listen_value() {
        assert!(matches_protocol("x:1 (Stream)", Protocol::Tcp));
        assert!(!matches_protocol("x:1 (Stream)", Protocol::Udp));
        assert!(matches_protocol("x:1 (Datagram)", Protocol::Udp));
    }

    #[test]
    fn user_socket_unit_found_by_listen_when_cgroup_is_init_scope() {
        let endpoints = vec![Endpoint {
            protocol: Protocol::Tcp,
            state: EndpointState::TcpListen,
            local_addr: "127.0.0.1".into(),
            scope: None,
            local_port: 39873,
            ip_version: IpVersion::V4,
            inode: Some(7),
            uid: Some(1000),
            cgroup: Some("/user.slice/user-1000.slice/user@1000.service/init.scope".into()),
        }];
        let services = vec![ServiceInfo {
            id: "psprobe.socket".into(),
            scope: UnitScope::User,
            kind: UnitKind::Socket,
            active_state: ActiveState::Active,
            sub_state: "listening".into(),
            load_state: "loaded".into(),
            main_pid: None,
            control_group: Some(
                "/user.slice/user-1000.slice/user@1000.service/app.slice/psprobe.socket".into(),
            ),
            listen: vec!["127.0.0.1:39873 (Stream)".into()],
            triggers: vec!["psprobe.service".into()],
            triggered_by: vec![],
            fragment_path: None,
            source: ServiceSource::Systemctl,
        }];
        let (assoc, _e) = associate_endpoints_to_units(&endpoints, &services);
        assert_eq!(assoc.len(), 1);
        match &assoc[0].kind {
            AssociationKind::EndpointToService { unit, .. } => assert_eq!(unit, "psprobe.service"),
            _ => panic!("wrong kind"),
        }
        assert!(
            assoc[0]
                .evidence
                .iter()
                .any(|e| e.kind == EvidenceKind::SocketActivation)
        );
    }

    #[test]
    fn inactive_socket_unit_does_not_grab_endpoint() {
        let endpoints = vec![Endpoint {
            protocol: Protocol::Tcp,
            state: EndpointState::TcpListen,
            local_addr: "127.0.0.1".into(),
            scope: None,
            local_port: 39873,
            ip_version: IpVersion::V4,
            inode: Some(7),
            uid: Some(1000),
            cgroup: Some("/user.slice/user-1000.slice/user@1000.service/init.scope".into()),
        }];
        let services = vec![ServiceInfo {
            id: "psprobe.socket".into(),
            scope: UnitScope::User,
            kind: UnitKind::Socket,
            active_state: ActiveState::Inactive,
            sub_state: "dead".into(),
            load_state: "loaded".into(),
            main_pid: None,
            control_group: None,
            listen: vec!["127.0.0.1:39873 (Stream)".into()],
            triggers: vec!["psprobe.service".into()],
            triggered_by: vec![],
            fragment_path: None,
            source: ServiceSource::Systemctl,
        }];
        let (assoc, _e) = associate_endpoints_to_units(&endpoints, &services);
        // The inactive socket unit must not be credited. init.scope is not a
        // service, so nothing should be associated at all.
        assert!(
            assoc.is_empty(),
            "inactive socket unit must not be credited: {:#?}",
            assoc
        );
    }

    /// Regression: the cgroup join used to be skipped whenever an `ss -p` hint
    /// had already produced an identity, and it was never consulted at all when
    /// the hint pointed at a process we could not inspect. The result was that
    /// endpoints whose cgroup *unambiguously* named an owner were reported as
    /// "permission denied" — 124 of 141 endpoints on a real machine.
    #[test]
    fn cgroup_join_runs_even_when_a_pid_hint_exists() {
        let e = ep(
            Protocol::Udp,
            "172.30.0.1",
            9993,
            Some("/system.slice/zerotier-one.service"),
            Some(7),
        );
        let p = ProcessInfo {
            pid: 3262,
            name: "zerotier-one".into(),
            cmdline: None,
            uid: Some(997),
            user: Some("zerotier-one".into()),
            ppid: Some(1),
            start_time_ticks: Some(2140),
            source: PidSource::CgroupProcs,
        };
        // An `ss -p` hint for a pid we never managed to inspect.
        let mut hints = std::collections::HashMap::new();
        hints.insert(e.key(), 99_999u32);
        let inode_hits = std::collections::HashMap::new();
        let mut cgroups = std::collections::HashMap::new();
        cgroups.insert(3262u32, "/system.slice/zerotier-one.service".to_string());

        let out = associate_endpoints_to_processes(&[e], &[p], &hints, &inode_hits, &cgroups);
        assert_eq!(out.len(), 1, "cgroup must still resolve the owner");
        match &out[0].kind {
            AssociationKind::EndpointToProcess { identity, .. } => {
                assert_eq!(identity.pid, 3262);
            }
            other => panic!("wrong kind: {other:?}"),
        }
        assert!(
            out[0]
                .evidence
                .iter()
                .any(|x| x.kind == EvidenceKind::CgroupUnitMatch),
            "the cgroup evidence must be recorded"
        );
    }

    #[test]
    fn error_codes_are_stable() {
        assert_eq!(
            CollectionError::ToolMissing { tool: "ss".into() }.code(),
            "tool_missing"
        );
        assert_eq!(
            CollectionError::JournalPermissionDenied { hint: "x".into() }.code(),
            "journal_permission_denied"
        );
        assert_eq!(
            CollectionError::NoJournalEntries { unit: None }.code(),
            "no_journal_entries"
        );
    }
}
