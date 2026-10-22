//! Bounded, privacy-conscious diagnostic summary.
//!
//! The summary is what a user pastes into a bug report. It must therefore:
//!
//! * be **bounded** — a fixed number of rows, never a full dump;
//! * **exclude full command lines**, which routinely carry passwords, tokens
//!   and private paths (taskbook §5.4, §7.3);
//! * carry the **collection time and degraded flags**, so the reader knows what
//!   was missing and how old it is;
//! * keep raw system values verbatim — but only the ones that help diagnosis.

use portscope_core::model::CollectionError;

use crate::state::{AppState, Freshness, LoadState};

/// Maximum endpoints / processes / services included.
const MAX_ENDPOINTS: usize = 40;
const MAX_PROCESSES: usize = 20;
const MAX_SERVICES: usize = 20;

/// Render the summary text.
pub fn diagnostic_summary(state: &AppState) -> String {
    let mut out = String::new();
    out.push_str("# PortScope diagnostic summary\n");

    // Provenance first: a summary without a timestamp is not evidence.
    match state.collected_at {
        Some(t) => {
            let secs = t
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            out.push_str(&format!("collected_at_unix: {secs}\n"));
        }
        None => out.push_str("collected_at_unix: (never collected)\n"),
    }
    out.push_str(&format!("freshness: {}\n", freshness_key(state.freshness)));
    out.push_str(&format!("load_state: {}\n", load_key(&state.load)));
    out.push_str(&format!(
        "counts: endpoints={} processes={} services={}\n",
        state.ports.len(),
        state.processes.len(),
        state.services.len()
    ));
    out.push_str(&format!(
        "endpoints_without_process: {}\n",
        state.unowned_endpoint_count()
    ));

    // Errors, by stable code — these are the machine-readable part.
    if let Some(snap) = &state.snapshot {
        if snap.errors.is_empty() {
            out.push_str("errors: none\n");
        } else {
            out.push_str("errors:\n");
            let mut seen: Vec<&str> = Vec::new();
            for e in &snap.errors {
                let code = e.code();
                if !seen.contains(&code) {
                    seen.push(code);
                    out.push_str(&format!("  - {code}\n"));
                }
            }
            out.push_str(&format!("  (total {} occurrences)\n", snap.errors.len()));
        }
        // Permit-related hints are worth spelling out; they are fixed text.
        for e in &snap.errors {
            if let CollectionError::PermissionDenied { resource, .. } = e {
                out.push_str(&format!("  permission_denied_for: {resource}\n"));
            }
        }
    }

    out.push_str("\n## endpoints (protocol/state/address:port/uid/pid/unit)\n");
    for p in state.ports.iter().take(MAX_ENDPOINTS) {
        out.push_str(&format!(
            "{}\t{}\t{}:{}\tuid={}\tpid={}\tunit={}\n",
            p.protocol.as_str(),
            p.state.label(),
            p.local_addr,
            p.port,
            opt(p.uid.map(|u| u.to_string())),
            opt(p.pid.map(|v| v.to_string())),
            p.unit.as_deref().unwrap_or("-"),
        ));
        // Evidence lines make the report actionable without guessing.
        for ev in p.evidence.iter().take(2) {
            out.push_str(&format!("    evidence: {ev}\n"));
        }
    }
    if state.ports.len() > MAX_ENDPOINTS {
        out.push_str(&format!(
            "... {} more endpoints omitted\n",
            state.ports.len() - MAX_ENDPOINTS
        ));
    }

    out.push_str("\n## processes (name/pid/user/endpoints)\n");
    for p in state.processes.iter().take(MAX_PROCESSES) {
        out.push_str(&format!(
            "{}\tpid={}\tuser={}\tendpoints={}\n",
            p.name,
            p.pid,
            p.user
                .clone()
                .or_else(|| p.uid.map(|u| u.to_string()))
                .unwrap_or_else(|| "?".into()),
            p.endpoint_count,
        ));
        // Command lines are DELIBERATELY omitted (see module docs).
    }
    if state.processes.len() > MAX_PROCESSES {
        out.push_str(&format!(
            "... {} more processes omitted\n",
            state.processes.len() - MAX_PROCESSES
        ));
    }

    out.push_str("\n## services (unit/scope/state/sub)\n");
    for s in state.services.iter().take(MAX_SERVICES) {
        out.push_str(&format!(
            "{}\t{}\t{}\t{}\n",
            s.unit,
            match s.scope {
                portscope_core::model::UnitScope::System => "system",
                portscope_core::model::UnitScope::User => "user",
            },
            s.active_state.as_str(),
            s.sub_state,
        ));
    }
    if state.services.len() > MAX_SERVICES {
        out.push_str(&format!(
            "... {} more services omitted\n",
            state.services.len() - MAX_SERVICES
        ));
    }

    out.push_str("\n# command lines are omitted from this summary\n");
    out
}

fn opt(v: Option<String>) -> String {
    v.unwrap_or_else(|| "-".into())
}

fn freshness_key(f: Freshness) -> &'static str {
    match f {
        Freshness::Fresh => "fresh",
        Freshness::Stale => "stale",
        Freshness::Unknown => "unknown",
    }
}

fn load_key(l: &LoadState) -> &'static str {
    match l {
        LoadState::Idle => "idle",
        LoadState::Loading => "loading",
        LoadState::Ok => "ok",
        LoadState::Failed(_) => "failed",
        LoadState::Cancelled => "cancelled",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;
    use portscope_core::model::*;

    fn ep(port: u16, cgroup: Option<&str>) -> Endpoint {
        Endpoint {
            protocol: Protocol::Tcp,
            state: EndpointState::TcpListen,
            local_addr: "0.0.0.0".into(),
            scope: None,
            local_port: port,
            ip_version: IpVersion::V4,
            inode: Some(1),
            uid: Some(1000),
            cgroup: cgroup.map(str::to_string),
        }
    }

    #[test]
    fn summary_is_bounded_and_states_counts() {
        let mut snap = Snapshot::default();
        for p in 0..200u16 {
            snap.endpoints.push(ep(1000 + p, None));
        }
        let mut st = AppState::default();
        st.apply_snapshot(snap);
        let s = diagnostic_summary(&st);
        assert_eq!(
            s.lines().filter(|l| l.starts_with("tcp\t")).count(),
            MAX_ENDPOINTS
        );
        assert!(s.contains("more endpoints omitted"));
        assert!(s.contains("counts: endpoints=200"));
    }

    #[test]
    fn summary_omits_command_lines_but_keeps_names() {
        let mut snap = Snapshot::default();
        snap.processes.push(ProcessInfo {
            pid: 9,
            name: "daemon".into(),
            cmdline: Some(vec!["daemon".into(), "--token=SEKRET".into()]),
            uid: Some(0),
            user: Some("root".into()),
            ppid: Some(1),
            start_time_ticks: Some(1),
            source: PidSource::CgroupProcs,
        });
        let mut st = AppState::default();
        st.apply_snapshot(snap);
        let s = diagnostic_summary(&st);
        assert!(!s.contains("SEKRET"));
        assert!(s.contains("daemon"));
        assert!(s.contains("command lines are omitted"));
    }

    #[test]
    fn summary_marks_stale_data() {
        let mut st = AppState::default();
        st.apply_snapshot(Snapshot::default());
        st.apply_failure("ss timed out");
        let s = diagnostic_summary(&st);
        assert!(s.contains("freshness: stale"));
        assert!(s.contains("load_state: failed"));
    }

    #[test]
    fn summary_lists_error_codes_once() {
        let mut snap = Snapshot::default();
        snap.errors.push(CollectionError::PermissionDenied {
            resource: "x".into(),
            hint: "y".into(),
        });
        snap.errors.push(CollectionError::PermissionDenied {
            resource: "z".into(),
            hint: "w".into(),
        });
        let mut st = AppState::default();
        st.apply_snapshot(snap);
        let s = diagnostic_summary(&st);
        assert_eq!(s.matches("- permission_denied").count(), 1);
        assert!(s.contains("total 2 occurrences"));
    }

    #[test]
    fn summary_never_collected_is_explicit() {
        let st = AppState::default();
        let s = diagnostic_summary(&st);
        assert!(s.contains("(never collected)"));
        assert!(s.contains("freshness: unknown"));
    }
}
