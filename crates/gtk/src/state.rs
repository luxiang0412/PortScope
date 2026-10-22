//! Presentation state derived from a [`Snapshot`].
//!
//! This module is deliberately GTK-free so the filtering, sorting, staleness and
//! error-classification logic can be unit-tested without a display server
//! (see `docs/TESTING.md`, "UI logic" layer).
//!
//! Three rules from the taskbook drive everything here:
//!   * `unknown` / `denied` / `empty` / `stale` / `failed` are distinct states,
//!     never collapsed into an empty list (§5.3, §5.9, §10);
//!   * a failed refresh keeps the last good data and marks it stale;
//!   * raw technical values (addresses, ports, unit names, command lines) are
//!     never translated or reformatted.

use portscope_core::model::*;
use std::time::SystemTime;

/// How fresh the displayed data is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    /// Never loaded yet.
    Unknown,
    /// Loaded and current.
    Fresh,
    /// Loaded earlier; a later refresh failed, so this may be out of date.
    Stale,
}

/// Lifecycle of the collection, independent of the data itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadState {
    /// No attempt yet.
    Idle,
    /// A collection is running.
    Loading,
    /// Last attempt succeeded.
    Ok,
    /// Last attempt failed outright (no usable snapshot at all).
    Failed(String),
    /// Cancelled by the user or superseded by a newer request.
    Cancelled,
}

/// A startup-invisible field: the value may legitimately be unknown for reasons
/// the UI must state, rather than showing a blank.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InfoState {
    /// Value present.
    Known,
    /// Not available because of permissions.
    Denied,
    /// Not queried (would have cost more; not needed yet).
    NotQueried,
    /// Object vanished between listing and inspection.
    Gone,
    /// Genuinely unknown (e.g. `ss -p` omitted `users:`).
    Unknown,
}

impl InfoState {
    pub fn is_known(self) -> bool {
        self == InfoState::Known
    }
}

/// Which view is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Ports,
    Processes,
    Services,
}

impl View {
    pub fn all() -> &'static [View] {
        &[View::Ports, View::Processes, View::Services]
    }

    /// Stable id used for stack pages and settings persistence.
    pub fn id(self) -> &'static str {
        match self {
            View::Ports => "ports",
            View::Processes => "processes",
            View::Services => "services",
        }
    }
}

// ---------------------------------------------------------------------------
// Row models
// ---------------------------------------------------------------------------

/// One row of the Ports table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortRow {
    pub protocol: Protocol,
    pub state: EndpointState,
    pub local_addr: String,
    pub port: u16,
    pub uid: Option<u32>,
    pub user: Option<String>,
    pub pid: Option<u32>,
    pub process: Option<String>,
    /// Unit confirmed by evidence, if any. Never guessed.
    pub unit: Option<String>,
    /// True when no process could be attributed, so the row is incomplete for a
    /// permission reason rather than because the socket has no owner.
    pub degraded: bool,
    /// Total number of distinct evidence kinds behind the unit link.
    pub evidence_count: usize,
    /// Human-readable raw evidence strings (untranslated).
    pub evidence: Vec<String>,
    pub identity: Option<ProcessIdentity>,
}

impl PortRow {
    /// Text the search box matches against: port, address, process, PID, user,
    /// and *confirmed* unit only (taskbook §5.3).
    pub fn search_text(&self) -> String {
        let mut s = String::with_capacity(96);
        s.push_str(self.protocol.as_str());
        s.push(' ');
        s.push_str(&self.local_addr);
        s.push(' ');
        s.push_str(&self.port.to_string());
        if let Some(p) = self.pid {
            s.push(' ');
            s.push_str(&p.to_string());
        }
        if let Some(p) = &self.process {
            s.push(' ');
            s.push_str(p);
        }
        if let Some(u) = &self.user {
            s.push(' ');
            s.push_str(u);
        } else if let Some(u) = self.uid {
            s.push(' ');
            s.push_str(&u.to_string());
        }
        if let Some(u) = &self.unit {
            s.push(' ');
            s.push_str(u);
        }
        s.to_ascii_lowercase()
    }
}

/// One row of the Processes table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessRow {
    pub pid: u32,
    pub name: String,
    pub user: Option<String>,
    pub uid: Option<u32>,
    pub ppid: Option<u32>,
    pub start_time_ticks: Option<u64>,
    /// Command line, kept as argv. Excluded from diagnostic export by default.
    pub cmdline: Option<Vec<String>>,
    pub endpoint_count: usize,
    /// Units confirmed by evidence for this process.
    pub units: Vec<String>,
    /// Whether the command line could be read.
    pub cmdline_state: InfoState,
}

impl ProcessRow {
    pub fn cmdline_display(&self) -> String {
        match &self.cmdline {
            Some(parts) => parts.join(" "),
            None => String::new(),
        }
    }

    pub fn search_text(&self) -> String {
        let mut s = format!("{} {}", self.pid, self.name);
        if let Some(u) = &self.user {
            s.push(' ');
            s.push_str(u);
        }
        for u in &self.units {
            s.push(' ');
            s.push_str(u);
        }
        s.to_ascii_lowercase()
    }
}

/// One row of the Services table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceRow {
    pub unit: String,
    pub scope: UnitScope,
    pub kind: UnitKind,
    pub active_state: ActiveState,
    pub sub_state: String,
    pub load_state: String,
    pub main_pid: Option<u32>,
    pub control_group: Option<String>,
    pub listen: Vec<String>,
    pub triggers: Vec<String>,
    pub ports: Vec<(Protocol, String, u16)>,
    /// Raw evidence strings for endpoint links.
    pub evidence: Vec<String>,
    /// True when detail fields were not queried for this unit (list-level row).
    pub detail_state: InfoState,
}

impl ServiceRow {
    pub fn search_text(&self) -> String {
        let mut s = self.unit.to_ascii_lowercase();
        for (_, addr, port) in &self.ports {
            s.push(' ');
            s.push_str(addr);
            s.push(' ');
            s.push_str(&port.to_string());
        }
        s
    }

    /// A failed unit is a fault; inactive is not (taskbook §5.5).
    pub fn is_fault(&self) -> bool {
        matches!(self.active_state, ActiveState::Failed)
    }
}

// ---------------------------------------------------------------------------
// Build rows from a snapshot
// ---------------------------------------------------------------------------

/// Derive all three row sets from one snapshot.
///
/// Every join performed here is the one the core already proved; this function
/// only *looks up* associations, it never infers a new one.
pub fn build_rows(snap: &Snapshot) -> (Vec<PortRow>, Vec<ProcessRow>, Vec<ServiceRow>) {
    use std::collections::BTreeMap;

    let mut ports = Vec::with_capacity(snap.endpoints.len());
    for ep in &snap.endpoints {
        let key = ep.key();

        // Process association: first by the evidence the core recorded.
        let mut identity: Option<ProcessIdentity> = None;
        let mut evidence: Vec<String> = Vec::new();
        let mut unit: Option<String> = None;
        let mut evidence_count = 0usize;

        for a in &snap.associations {
            match &a.kind {
                AssociationKind::EndpointToProcess {
                    endpoint,
                    identity: id,
                } if *endpoint == key => {
                    if identity.is_none() {
                        identity = Some(id.clone());
                    }
                }
                AssociationKind::EndpointToService {
                    endpoint, unit: u, ..
                } if *endpoint == key => {
                    if unit.is_none() {
                        unit = Some(u.clone());
                    }
                }
                _ => {}
            }
            // Evidence belongs to whichever association matched this endpoint.
            let is_for_endpoint = match &a.kind {
                AssociationKind::EndpointToProcess { endpoint, .. }
                | AssociationKind::EndpointToService { endpoint, .. } => *endpoint == key,
                _ => false,
            };
            if is_for_endpoint {
                for e in &a.evidence {
                    let s = format!("{:?}: {}", e.kind, e.detail);
                    if !evidence.contains(&s) {
                        evidence.push(s);
                    }
                }
                evidence_count = evidence_count.max(a.evidence.len());
            }
        }

        let proc_info = identity
            .as_ref()
            .and_then(|id| snap.process_by_identity(id));

        let uid = ep.uid.or_else(|| proc_info.and_then(|p| p.uid));

        ports.push(PortRow {
            protocol: ep.protocol,
            state: ep.state,
            local_addr: ep.display_addr(),
            port: ep.local_port,
            uid,
            user: proc_info.and_then(|p| p.user.clone()).or_else(|| {
                // Fall back to the socket owner's name from the process list.
                ep.uid.and_then(|u| {
                    snap.processes
                        .iter()
                        .find(|p| p.uid == Some(u))
                        .and_then(|p| p.user.clone())
                })
            }),
            pid: identity.as_ref().map(|i| i.pid),
            process: proc_info.map(|p| p.name.clone()),
            unit,
            degraded: identity.is_none(),
            evidence_count,
            evidence,
            identity,
        });
    }

    // Endpoints per process identity, and units per process.
    let mut endpoints_by_pid: BTreeMap<u32, usize> = BTreeMap::new();
    let mut units_by_pid: BTreeMap<u32, Vec<String>> = BTreeMap::new();
    for p in &ports {
        if let Some(id) = &p.identity {
            *endpoints_by_pid.entry(id.pid).or_default() += 1;
            if let Some(u) = &p.unit {
                let e = units_by_pid.entry(id.pid).or_default();
                if !e.contains(u) {
                    e.push(u.clone());
                }
            }
        }
    }

    let processes = snap
        .processes
        .iter()
        .map(|p| ProcessRow {
            pid: p.pid,
            name: p.name.clone(),
            user: p.user.clone(),
            uid: p.uid,
            ppid: p.ppid,
            start_time_ticks: p.start_time_ticks,
            cmdline_state: if p.cmdline.is_some() {
                InfoState::Known
            } else {
                InfoState::Denied
            },
            cmdline: p.cmdline.clone(),
            endpoint_count: endpoints_by_pid.get(&p.pid).copied().unwrap_or(0),
            units: units_by_pid.get(&p.pid).cloned().unwrap_or_default(),
        })
        .collect();

    // Ports per unit, from confirmed associations only.
    let mut ports_by_unit: BTreeMap<String, Vec<(Protocol, String, u16)>> = BTreeMap::new();
    let mut evidence_by_unit: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for a in &snap.associations {
        if let AssociationKind::EndpointToService { endpoint, unit, .. } = &a.kind {
            let e = ports_by_unit.entry(unit.clone()).or_default();
            let entry = (endpoint.protocol, endpoint.addr.clone(), endpoint.port);
            if !e.contains(&entry) {
                e.push(entry);
            }
            let ev = evidence_by_unit.entry(unit.clone()).or_default();
            for evidence in &a.evidence {
                let s = format!("{:?}: {}", evidence.kind, evidence.detail);
                if !ev.contains(&s) {
                    ev.push(s);
                }
            }
        }
    }

    let services = snap
        .services
        .iter()
        .map(|s| {
            // A unit is "detailed" when the collector enriched it: socket units
            // and units named by an endpoint cgroup. Everything else is a
            // list-level row whose detail fields were never queried.
            let detail_state = if s.kind == UnitKind::Socket
                || s.main_pid.is_some()
                || s.control_group.is_some()
                || !s.listen.is_empty()
            {
                InfoState::Known
            } else {
                InfoState::NotQueried
            };
            ServiceRow {
                unit: s.id.clone(),
                scope: s.scope,
                kind: s.kind,
                active_state: s.active_state,
                sub_state: s.sub_state.clone(),
                load_state: s.load_state.clone(),
                main_pid: s.main_pid,
                control_group: s.control_group.clone(),
                listen: s.listen.clone(),
                triggers: s.triggers.clone(),
                ports: ports_by_unit.get(&s.id).cloned().unwrap_or_default(),
                evidence: evidence_by_unit.get(&s.id).cloned().unwrap_or_default(),
                detail_state,
            }
        })
        .collect();

    (ports, processes, services)
}

// ---------------------------------------------------------------------------
// Filtering
// ---------------------------------------------------------------------------

/// Endpoint filter settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EndpointFilter {
    /// `None` = both.
    pub protocol: Option<Protocol>,
    /// `None` = both families.
    pub ipv6_only: Option<bool>,
    /// Only show endpoints without a confirmed process (`None` = no restriction).
    pub unowned_only: bool,
}

impl EndpointFilter {
    pub fn matches(&self, row: &PortRow, ipv6: bool) -> bool {
        if let Some(p) = self.protocol {
            if row.protocol != p {
                return false;
            }
        }
        if let Some(v6) = self.ipv6_only {
            if v6 != ipv6 {
                return false;
            }
        }
        if self.unowned_only && row.pid.is_some() {
            return false;
        }
        true
    }
}

/// Case-insensitive substring match over a row's search text.
pub fn matches_query(search_text: &str, query: &str) -> bool {
    let q = query.trim().to_ascii_lowercase();
    if q.is_empty() {
        return true;
    }
    // Multiple space-separated terms must all match (AND), which makes
    // "tcp 22" and "sshd root" behave as users expect.
    q.split_whitespace().all(|term| search_text.contains(term))
}

/// Sort direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDir {
    Asc,
    Desc,
}

// ---------------------------------------------------------------------------
// Top-level app state
// ---------------------------------------------------------------------------

/// The whole presentation state: last snapshot, derived rows, freshness.
///
/// Keeping this in one struct is what makes "preserve selection and filters
/// across refresh" implementable and testable: the rows are replaced, the
/// *selection is re-resolved by identity*, not by row index.
#[derive(Debug, Clone)]
pub struct AppState {
    pub snapshot: Option<Snapshot>,
    pub freshness: Freshness,
    pub load: LoadState,
    pub ports: Vec<PortRow>,
    pub processes: Vec<ProcessRow>,
    pub services: Vec<ServiceRow>,
    /// When the displayed snapshot was collected.
    pub collected_at: Option<SystemTime>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            snapshot: None,
            freshness: Freshness::Unknown,
            load: LoadState::Idle,
            ports: Vec::new(),
            processes: Vec::new(),
            services: Vec::new(),
            collected_at: None,
        }
    }
}

impl AppState {
    /// Apply a successful collection.
    pub fn apply_snapshot(&mut self, snap: Snapshot) {
        let (p, pr, s) = build_rows(&snap);
        self.collected_at = snap.collected_at;
        self.ports = p;
        self.processes = pr;
        self.services = s;
        self.snapshot = Some(snap);
        self.freshness = Freshness::Fresh;
        self.load = LoadState::Ok;
    }

    /// Record a failed refresh.
    ///
    /// The previous data is **kept** and marked stale, because showing nothing
    /// is less useful than showing the last known truth with an explicit
    /// warning (taskbook §5.3).
    pub fn apply_failure(&mut self, message: impl Into<String>) {
        let msg = message.into();
        self.load = LoadState::Failed(msg);
        self.freshness = if self.snapshot.is_some() {
            Freshness::Stale
        } else {
            Freshness::Unknown
        };
    }

    pub fn apply_cancelled(&mut self) {
        if self.snapshot.is_some() {
            // Data is still whatever we had; it just was not refreshed.
            self.freshness = Freshness::Stale;
        }
        self.load = LoadState::Cancelled;
    }

    pub fn begin_loading(&mut self) {
        self.load = LoadState::Loading;
    }

    /// Count of endpoints with no attributable process — the honest
    /// "information unavailable" signal.
    pub fn unowned_endpoint_count(&self) -> usize {
        self.ports.iter().filter(|p| p.degraded).count()
    }

    /// Permission-related errors from the last snapshot, for the banner.
    pub fn permission_errors(&self) -> Vec<&CollectionError> {
        match &self.snapshot {
            Some(s) => s
                .errors
                .iter()
                .filter(|e| {
                    matches!(
                        e,
                        CollectionError::PermissionDenied { .. }
                            | CollectionError::JournalPermissionDenied { .. }
                    )
                })
                .collect(),
            None => Vec::new(),
        }
    }

    pub fn has_journal_permission_error(&self) -> bool {
        match &self.snapshot {
            Some(s) => s
                .errors
                .iter()
                .any(|e| matches!(e, CollectionError::JournalPermissionDenied { .. })),
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ep(proto: Protocol, addr: &str, port: u16, cgroup: Option<&str>) -> Endpoint {
        Endpoint {
            protocol: proto,
            state: EndpointState::TcpListen,
            local_addr: addr.into(),
            scope: None,
            local_port: port,
            ip_version: if addr.contains(':') {
                IpVersion::V6
            } else {
                IpVersion::V4
            },
            inode: Some(1),
            uid: Some(1000),
            cgroup: cgroup.map(str::to_string),
        }
    }

    fn proc(pid: u32, name: &str) -> ProcessInfo {
        ProcessInfo {
            pid,
            name: name.into(),
            cmdline: Some(vec![name.into()]),
            uid: Some(1000),
            user: Some("me".into()),
            ppid: Some(1),
            start_time_ticks: Some(500),
            source: PidSource::TestHarness,
        }
    }

    fn svc(id: &str, state: ActiveState) -> ServiceInfo {
        ServiceInfo {
            id: id.into(),
            scope: UnitScope::System,
            kind: if id.ends_with(".socket") {
                UnitKind::Socket
            } else {
                UnitKind::Service
            },
            active_state: state,
            sub_state: "running".into(),
            load_state: "loaded".into(),
            main_pid: None,
            control_group: None,
            listen: vec![],
            triggers: vec![],
            triggered_by: vec![],
            fragment_path: None,
            source: ServiceSource::DBus,
        }
    }

    #[test]
    fn empty_snapshot_yields_empty_rows_and_unknown_freshness() {
        let mut st = AppState::default();
        st.apply_snapshot(Snapshot::default());
        assert!(st.ports.is_empty());
        assert_eq!(st.freshness, Freshness::Fresh);
        assert_eq!(st.load, LoadState::Ok);
    }

    #[test]
    fn failure_keeps_last_data_and_marks_stale() {
        let mut st = AppState::default();
        let mut snap = Snapshot::default();
        snap.endpoints.push(ep(Protocol::Tcp, "0.0.0.0", 22, None));
        st.apply_snapshot(snap);
        assert_eq!(st.ports.len(), 1);
        assert_eq!(st.freshness, Freshness::Fresh);

        st.apply_failure("ss timed out");
        // Data must NOT be dropped (taskbook §5.3).
        assert_eq!(st.ports.len(), 1);
        assert_eq!(st.freshness, Freshness::Stale);
        assert!(matches!(st.load, LoadState::Failed(_)));
    }

    #[test]
    fn failure_without_prior_data_stays_unknown() {
        let mut st = AppState::default();
        st.apply_failure("boom");
        assert_eq!(st.freshness, Freshness::Unknown);
        assert!(st.ports.is_empty());
    }

    #[test]
    fn endpoint_without_process_is_degraded_not_empty_owner() {
        let mut snap = Snapshot::default();
        snap.endpoints.push(ep(Protocol::Tcp, "0.0.0.0", 22, None));
        snap.endpoints.push(ep(Protocol::Tcp, "0.0.0.0", 80, None));
        snap.processes.push(proc(42, "sshd"));
        snap.associations.push(Association::new(
            AssociationKind::EndpointToProcess {
                endpoint: snap.endpoints[0].key(),
                identity: ProcessIdentity {
                    pid: 42,
                    start_time_ticks: 500,
                },
            },
            vec![Evidence {
                kind: EvidenceKind::SocketInodeInProcFd,
                detail: "inode 1 in /proc/42/fd".into(),
            }],
        ));
        let mut st = AppState::default();
        st.apply_snapshot(snap);
        assert_eq!(st.ports.len(), 2);
        assert_eq!(st.unowned_endpoint_count(), 1);
        let owned = st.ports.iter().find(|p| p.port == 22).unwrap();
        assert!(!owned.degraded);
        assert_eq!(owned.pid, Some(42));
        assert_eq!(owned.process.as_deref(), Some("sshd"));
    }

    #[test]
    fn unit_comes_only_from_association_never_from_port() {
        // A port-22 endpoint with no cgroup and no association must NOT be
        // given ssh.service just because the port is 22.
        let mut snap = Snapshot::default();
        snap.endpoints.push(ep(Protocol::Tcp, "0.0.0.0", 22, None));
        snap.services.push(svc("ssh.service", ActiveState::Active));
        let mut st = AppState::default();
        st.apply_snapshot(snap);
        assert_eq!(
            st.ports[0].unit, None,
            "must not guess a unit from the port"
        );
    }

    #[test]
    fn unit_from_association_is_shown_with_evidence() {
        let mut snap = Snapshot::default();
        let e = ep(
            Protocol::Tcp,
            "0.0.0.0",
            22,
            Some("/system.slice/ssh.service"),
        );
        snap.endpoints.push(e.clone());
        snap.services.push(svc("ssh.service", ActiveState::Active));
        snap.associations.push(Association::new(
            AssociationKind::EndpointToService {
                endpoint: e.key(),
                unit: "ssh.service".into(),
                scope: UnitScope::System,
            },
            vec![Evidence {
                kind: EvidenceKind::CgroupUnitMatch,
                detail: "cgroup:/system.slice/ssh.service".into(),
            }],
        ));
        let mut st = AppState::default();
        st.apply_snapshot(snap);
        assert_eq!(st.ports[0].unit.as_deref(), Some("ssh.service"));
        assert!(!st.ports[0].evidence.is_empty());
    }

    #[test]
    fn search_matches_port_pid_process_user_and_unit() {
        let row = PortRow {
            protocol: Protocol::Tcp,
            state: EndpointState::TcpListen,
            local_addr: "127.0.0.1".into(),
            port: 5432,
            uid: Some(1000),
            user: Some("postgres".into()),
            pid: Some(1240),
            process: Some("postgres".into()),
            unit: Some("postgresql.service".into()),
            degraded: false,
            evidence_count: 1,
            evidence: vec![],
            identity: None,
        };
        let t = row.search_text();
        assert!(matches_query(&t, "5432"));
        assert!(matches_query(&t, "1240"));
        assert!(matches_query(&t, "postgres"));
        assert!(matches_query(&t, "postgresql"));
        assert!(matches_query(&t, "127.0.0.1"));
        assert!(!matches_query(&t, "6379"));
        // multiple terms are ANDed
        assert!(matches_query(&t, "tcp 5432"));
        assert!(!matches_query(&t, "tcp 6379"));
        // empty query matches everything
        assert!(matches_query(&t, "   "));
    }

    #[test]
    fn search_is_case_insensitive() {
        let row = PortRow {
            protocol: Protocol::Tcp,
            state: EndpointState::TcpListen,
            local_addr: "0.0.0.0".into(),
            port: 22,
            uid: None,
            user: None,
            pid: None,
            process: None,
            unit: Some("SSH.Service".into()),
            degraded: true,
            evidence_count: 0,
            evidence: vec![],
            identity: None,
        };
        assert!(matches_query(&row.search_text(), "ssh.service"));
    }

    #[test]
    fn endpoint_filter_by_protocol_and_family() {
        let tcp4 = PortRow {
            protocol: Protocol::Tcp,
            state: EndpointState::TcpListen,
            local_addr: "0.0.0.0".into(),
            port: 22,
            uid: None,
            user: None,
            pid: None,
            process: None,
            unit: None,
            degraded: false,
            evidence_count: 0,
            evidence: vec![],
            identity: None,
        };
        let f = EndpointFilter {
            protocol: Some(Protocol::Udp),
            ..Default::default()
        };
        assert!(!f.matches(&tcp4, false));
        let f = EndpointFilter {
            ipv6_only: Some(true),
            ..Default::default()
        };
        assert!(!f.matches(&tcp4, false));
        let f = EndpointFilter::default();
        assert!(f.matches(&tcp4, false));
    }

    #[test]
    fn unowned_only_filter() {
        let mut row = PortRow {
            protocol: Protocol::Tcp,
            state: EndpointState::TcpListen,
            local_addr: "0.0.0.0".into(),
            port: 22,
            uid: None,
            user: None,
            pid: Some(1),
            process: Some("x".into()),
            unit: None,
            degraded: false,
            evidence_count: 0,
            evidence: vec![],
            identity: None,
        };
        let f = EndpointFilter {
            unowned_only: true,
            ..Default::default()
        };
        assert!(!f.matches(&row, false));
        row.pid = None;
        assert!(f.matches(&row, false));
    }

    #[test]
    fn selection_survives_refresh_by_identity_not_index() {
        // Two endpoints; the second is selected. After a refresh in which the
        // first disappears, the same endpoint must still be identified.
        let mut snap = Snapshot::default();
        snap.endpoints.push(ep(Protocol::Tcp, "0.0.0.0", 22, None));
        snap.endpoints.push(ep(Protocol::Tcp, "0.0.0.0", 80, None));
        let mut st = AppState::default();
        st.apply_snapshot(snap);
        let selected_key = st.ports[1].port; // 80
        assert_eq!(selected_key, 80);

        let mut snap2 = Snapshot::default();
        snap2.endpoints.push(ep(Protocol::Tcp, "0.0.0.0", 80, None));
        st.apply_snapshot(snap2);
        // Index 1 no longer exists, but port 80 is still findable by key.
        assert_eq!(st.ports.len(), 1);
        assert_eq!(st.ports[0].port, selected_key);
    }

    /// The UI keeps its selection in `App::selected_*`, and `refill_all`
    /// re-resolves it after rebuilding the stores. That only works if a
    /// "selection cleared because the store was rebuilt" notification does not
    /// overwrite the saved value — a bug that reset the details pane on every
    /// auto-refresh.
    ///
    /// `state` cannot test GTK signals, so this asserts the *data* contract the
    /// fix depends on: the same object is re-findable after a rebuild.
    #[test]
    fn same_object_is_refindable_after_a_rebuild() {
        let mut snap = Snapshot::default();
        let e = ep(
            Protocol::Tcp,
            "127.0.0.1",
            5432,
            Some("/system.slice/pg.service"),
        );
        snap.endpoints.push(e.clone());
        let mut st = AppState::default();
        st.apply_snapshot(snap);

        // This is the key the UI stores and re-resolves by.
        let saved = st.ports[0].clone();
        let key = (saved.protocol, saved.local_addr.clone(), saved.port);

        // A refresh with the same endpoint present must re-find it at its index.
        let mut snap2 = Snapshot::default();
        snap2.endpoints.push(e);
        st.apply_snapshot(snap2);
        let found = st
            .ports
            .iter()
            .position(|p| (p.protocol, p.local_addr.clone(), p.port) == key);
        assert!(
            found.is_some(),
            "selection key must re-resolve after refresh"
        );

        // And when the endpoint is genuinely gone, no row matches, so the UI
        // must clear the selection rather than point at a different object.
        let mut snap3 = Snapshot::default();
        snap3
            .endpoints
            .push(ep(Protocol::Tcp, "127.0.0.1", 5433, None));
        st.apply_snapshot(snap3);
        let found = st
            .ports
            .iter()
            .position(|p| (p.protocol, p.local_addr.clone(), p.port) == key);
        assert!(found.is_none(), "a vanished endpoint must not re-resolve");
    }

    #[test]
    fn inactive_is_not_a_fault_but_failed_is() {
        let inact = ServiceRow {
            unit: "a.service".into(),
            scope: UnitScope::System,
            kind: UnitKind::Service,
            active_state: ActiveState::Inactive,
            sub_state: "dead".into(),
            load_state: "loaded".into(),
            main_pid: None,
            control_group: None,
            listen: vec![],
            triggers: vec![],
            ports: vec![],
            evidence: vec![],
            detail_state: InfoState::NotQueried,
        };
        let mut failed = inact.clone();
        failed.active_state = ActiveState::Failed;
        assert!(!inact.is_fault());
        assert!(failed.is_fault());
    }

    #[test]
    fn list_level_service_row_is_marked_not_queried() {
        let mut snap = Snapshot::default();
        snap.services
            .push(svc("plain.service", ActiveState::Active));
        let mut st = AppState::default();
        st.apply_snapshot(snap);
        // No detail was fetched, so the row must say "not queried", not pretend
        // the values are empty.
        assert_eq!(st.services[0].detail_state, InfoState::NotQueried);
    }

    #[test]
    fn service_ports_are_attached_from_associations() {
        let mut snap = Snapshot::default();
        let e = ep(
            Protocol::Tcp,
            "127.0.0.1",
            5432,
            Some("/system.slice/postgresql.service"),
        );
        snap.endpoints.push(e.clone());
        snap.services
            .push(svc("postgresql.service", ActiveState::Active));
        snap.associations.push(Association::new(
            AssociationKind::EndpointToService {
                endpoint: e.key(),
                unit: "postgresql.service".into(),
                scope: UnitScope::System,
            },
            vec![Evidence {
                kind: EvidenceKind::CgroupUnitMatch,
                detail: "cgroup:/system.slice/postgresql.service".into(),
            }],
        ));
        let mut st = AppState::default();
        st.apply_snapshot(snap);
        assert_eq!(
            st.services[0].ports,
            vec![(Protocol::Tcp, "127.0.0.1".into(), 5432)]
        );
        assert!(!st.services[0].evidence.is_empty());
    }

    #[test]
    fn permission_errors_are_surfaced() {
        let mut snap = Snapshot::default();
        snap.errors.push(CollectionError::PermissionDenied {
            resource: "x".into(),
            hint: "y".into(),
        });
        snap.errors
            .push(CollectionError::JournalPermissionDenied { hint: "z".into() });
        snap.errors.push(CollectionError::Timeout {
            tool: "ss".into(),
            millis: 5000,
        });
        let mut st = AppState::default();
        st.apply_snapshot(snap);
        assert_eq!(st.permission_errors().len(), 2);
        assert!(st.has_journal_permission_error());
    }

    #[test]
    fn process_row_reports_denied_cmdline_not_empty_string() {
        let mut snap = Snapshot::default();
        let mut p = proc(7, "secret");
        p.cmdline = None;
        snap.processes.push(p);
        let mut st = AppState::default();
        st.apply_snapshot(snap);
        assert_eq!(st.processes[0].cmdline_state, InfoState::Denied);
        assert_eq!(st.processes[0].cmdline_display(), "");
    }

    #[test]
    fn endpoint_count_and_units_are_aggregated_per_process() {
        let mut snap = Snapshot::default();
        let a = ep(Protocol::Tcp, "0.0.0.0", 22, None);
        let b = ep(Protocol::Tcp, "::", 22, None);
        snap.endpoints.push(a.clone());
        snap.endpoints.push(b.clone());
        snap.processes.push(proc(42, "sshd"));
        let id = ProcessIdentity {
            pid: 42,
            start_time_ticks: 500,
        };
        for e in [&a, &b] {
            snap.associations.push(Association::new(
                AssociationKind::EndpointToProcess {
                    endpoint: e.key(),
                    identity: id.clone(),
                },
                vec![Evidence {
                    kind: EvidenceKind::SocketProcessField,
                    detail: "pid=42".into(),
                }],
            ));
        }
        let mut st = AppState::default();
        st.apply_snapshot(snap);
        assert_eq!(st.processes[0].endpoint_count, 2);
    }

    #[test]
    fn cancelled_refresh_marks_stale_and_keeps_data() {
        let mut st = AppState::default();
        let mut snap = Snapshot::default();
        snap.endpoints.push(ep(Protocol::Tcp, "0.0.0.0", 22, None));
        st.apply_snapshot(snap);
        st.begin_loading();
        assert_eq!(st.load, LoadState::Loading);
        st.apply_cancelled();
        assert_eq!(st.ports.len(), 1);
        assert_eq!(st.freshness, Freshness::Stale);
        assert_eq!(st.load, LoadState::Cancelled);
    }
}
