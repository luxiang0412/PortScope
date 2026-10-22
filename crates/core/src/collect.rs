//! Snapshot orchestration: one collect() call produces endpoints, processes,
//! services and associations with all partial failures attached.

use crate::assoc;
use crate::endpoints;
use crate::exec::Cancel;
use crate::model::*;
use crate::process as procmod;
use crate::services::{Backend, collect_services};
use std::collections::HashMap;
use std::time::SystemTime;

#[derive(Debug, Clone)]
pub struct CollectOptions {
    /// Try `ss -p` (only yields pids for our own processes).
    pub include_process_field: bool,
    /// Whether to enumerate system units.
    pub system_services: bool,
    /// Whether to enumerate the user manager's units.
    pub user_services: bool,
    /// Only inspect these unit names, if set.
    pub only_units: Option<Vec<String>>,
    pub backend: Backend,
    /// Upper bound on PIDs we will inspect (defensive).
    pub max_pids_inspected: usize,
}

impl Default for CollectOptions {
    fn default() -> Self {
        CollectOptions {
            include_process_field: true,
            system_services: true,
            user_services: true,
            only_units: None,
            // Phase 1 (audit P0-D5): D-Bus is the accepted primary path. The
            // Spike defaulted to Systemctl, contradicting decision D4, so the
            // product path never exercised D-Bus at all.
            backend: Backend::DBus,
            max_pids_inspected: 2000,
        }
    }
}

/// The cgroup root a unit of the given scope lives under.
///
/// Phase 1 (audit P0-D5): the Spike hardcoded `/system.slice/` for every unit,
/// so user-scope units were looked up under the wrong root and silently found
/// no processes.
fn unit_cgroup_root(scope: UnitScope) -> &'static str {
    match scope {
        UnitScope::System => "/system.slice",
        UnitScope::User => "/user.slice",
    }
}

pub fn collect(opts: &CollectOptions) -> Snapshot {
    collect_cancellable(opts, &Cancel::new())
}

pub fn collect_cancellable(opts: &CollectOptions, cancel: &Cancel) -> Snapshot {
    let mut snap = Snapshot {
        collected_at: Some(SystemTime::now()),
        ..Default::default()
    };

    // 1. endpoints
    let (rows, ep_errors) =
        endpoints::collect_endpoints_cancellable(opts.include_process_field, cancel);
    for e in ep_errors {
        snap.push_error(e);
    }
    let mut ss_pid_hints: HashMap<EndpointKey, u32> = HashMap::new();
    let mut endpoint_pids: Vec<u32> = Vec::new();
    for r in &rows {
        if let Some(pid) = r.pids.first() {
            ss_pid_hints.insert(r.endpoint.key(), *pid);
        }
        endpoint_pids.extend(r.pids.iter().copied());
    }
    let endpoints_vec: Vec<Endpoint> = rows.iter().map(|r| r.endpoint.clone()).collect();
    snap.endpoints = endpoints_vec;

    // 2. services
    let mut scopes = Vec::new();
    if opts.system_services {
        scopes.push(UnitScope::System);
    }
    if opts.user_services {
        scopes.push(UnitScope::User);
    }
    let (services, svc_errors) =
        collect_services(&scopes, opts.only_units.as_deref(), opts.backend);
    for e in svc_errors {
        snap.push_error(e);
    }
    // BUGFIX (found via live CLI run): services were collected but never
    // stored on the snapshot, which made every cgroup-derived unit look
    // "not found". Keep the list so associations can be verified against it.
    snap.services = services.clone();

    // 3. candidate PIDs: ss hints + MainPIDs + cgroup procs of socket-owning units
    let mut candidates: Vec<u32> = endpoint_pids.clone();
    for s in &services {
        if let Some(p) = s.main_pid {
            candidates.push(p);
        }
    }
    let mut cgroup_of_candidate: HashMap<u32, String> = HashMap::new();
    for s in &services {
        if s.kind == UnitKind::Socket {
            continue;
        }
        let cg = format!("{}/{}", unit_cgroup_root(s.scope), s.id);
        if let Ok(pids) = procmod::pids_in_cgroup(&cg) {
            for p in pids {
                candidates.push(p);
                cgroup_of_candidate.insert(p, cg.clone());
            }
        }
    }
    candidates.sort_unstable();
    candidates.dedup();
    if candidates.len() > opts.max_pids_inspected {
        candidates.truncate(opts.max_pids_inspected);
        snap.push_error(CollectionError::Truncated {
            limit: opts.max_pids_inspected,
        });
    }

    // 4. inspect
    let mut processes = Vec::new();
    for pid in &candidates {
        match procmod::inspect_process(*pid, PidSource::CgroupProcs, None) {
            Ok(p) => processes.push(p),
            Err(CollectionError::ProcessGone { .. }) => { /* normal race, ignore */ }
            Err(e) => snap.push_error(e),
        }
    }
    snap.processes = processes;

    // 5. socket inode ownership among candidates
    let inode_hits = procmod::inode_owners(&candidates);

    // 6. cgroup of every candidate PID (used for socket<->process joins)
    let mut cg_map: HashMap<u32, String> = cgroup_of_candidate;
    for pid in &candidates {
        if !cg_map.contains_key(pid) {
            if let Ok(text) = std::fs::read_to_string(format!("/proc/{}/cgroup", pid)) {
                if let Some(rest) = text.lines().find_map(|l| l.strip_prefix("0::")) {
                    cg_map.insert(*pid, rest.to_string());
                }
            }
        }
    }

    // 7. associations
    let (mut a1, a_errors) = assoc::associate_endpoints_to_units(&snap.endpoints, &snap.services);
    for e in a_errors {
        snap.push_error(e);
    }
    let a2 = assoc::associate_endpoints_to_processes(
        &snap.endpoints,
        &snap.processes,
        &ss_pid_hints,
        &inode_hits,
        &cg_map,
    );
    let a3 = assoc::associate_services_to_processes(&snap.services, &snap.processes);
    a1.extend(a2);
    a1.extend(a3);
    snap.associations = a1;

    // 8. degraded flag: endpoints we cannot attribute to a process.
    // This is the honest, visible signal for the normal-user limitation.
    let mut invisible_count = 0usize;
    let total = snap.endpoints.len();
    for e in &snap.endpoints {
        let has_pid = snap.associations.iter().any(|a| {
            matches!(&a.kind, AssociationKind::EndpointToProcess { endpoint, .. } if *endpoint == e.key())
        });
        if !has_pid {
            invisible_count += 1;
        }
    }
    if invisible_count > 0 && total > 0 {
        snap.degraded = true;
        snap.push_error(CollectionError::PermissionDenied {
            resource: "process info for some endpoints".into(),
            hint: format!(
                "{}/{} endpoints have no process association; PIDs for other users' processes require privilege",
                invisible_count, total
            ),
        });
    }

    snap
}
