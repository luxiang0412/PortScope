//! Pure data model. No GTK, no process invocation, no I/O.
//!
//! Design rule (Spike 01 §A5): an association is NEVER inferred from
//! port number, process name, or unit description. Every link carries the
//! `Evidence` that produced it, and every field that could not be read is
//! represented explicitly rather than as a default/empty value.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::SystemTime;

// ---------------------------------------------------------------------------
// Endpoint
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Protocol {
    Tcp,
    Udp,
    /// Raw sockets, or a netlink family we do not model. Kept so we do not
    /// silently drop rows we do not understand.
    Other,
}

impl Protocol {
    pub fn as_str(&self) -> &'static str {
        match self {
            Protocol::Tcp => "tcp",
            Protocol::Udp => "udp",
            Protocol::Other => "raw",
        }
    }
}

/// Socket state, normalised from kernel `ss`/`/proc` state codes.
///
/// `Unknown` is a first-class value: `ss` can emit state codes that are not
/// relevant to endpoints we care about, and we must not guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EndpointState {
    /// TCP `LISTEN` — accepting inbound connections.
    TcpListen,
    /// UDP bound & unconnected (`UNCONN`).
    UdpBound,
    /// UDP connected to a peer.
    UdpConnected,
    /// Anything else (ESTAB, TIME-WAIT, ...). Not an endpoint in the
    /// PortScope sense but retained with its raw code.
    Other(u8),
    /// We could not map the code.
    Unknown,
}

impl EndpointState {
    pub fn from_tcp_code(code: u8) -> EndpointState {
        match code {
            0x0A => EndpointState::TcpListen,
            other => EndpointState::Other(other),
        }
    }
    pub fn from_udp_code(code: u8) -> EndpointState {
        match code {
            0x07 => EndpointState::UdpBound,     // UDP UNCONN
            0x01 => EndpointState::UdpConnected, // UDP ESTAB
            other => EndpointState::Other(other),
        }
    }

    /// Parse the textual state emitted by `ss` (`LISTEN`, `UNCONN`, `ESTAB`, ...).
    pub fn from_ss_str(proto: Protocol, s: &str) -> EndpointState {
        match (proto, s) {
            (Protocol::Tcp, "LISTEN") => EndpointState::TcpListen,
            (Protocol::Udp, "UNCONN") => EndpointState::UdpBound,
            (Protocol::Udp, "ESTAB") => EndpointState::UdpConnected,
            (_, "UNKNOWN") => EndpointState::Unknown,
            _ => EndpointState::Unknown,
        }
    }

    /// Is this a *listening/bound* endpoint (what PortScope shows by default)?
    pub fn is_listening(&self) -> bool {
        matches!(
            self,
            EndpointState::TcpListen | EndpointState::UdpBound | EndpointState::UdpConnected
        )
    }

    pub fn label(&self) -> String {
        match self {
            EndpointState::TcpListen => "LISTEN".into(),
            EndpointState::UdpBound => "UNCONN".into(),
            EndpointState::UdpConnected => "ESTAB".into(),
            EndpointState::Other(c) => format!("STATE_{:#04x}", c),
            EndpointState::Unknown => "UNKNOWN".into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum IpVersion {
    V4,
    V6,
}

/// A local network endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Endpoint {
    pub protocol: Protocol,
    pub state: EndpointState,
    /// Address in canonical text form (`0.0.0.0`, `::`, `127.0.0.1`).
    pub local_addr: String,
    /// IPv6 scope id / interface name if `ss` reported one (e.g. `%enp4s0`).
    pub scope: Option<String>,
    pub local_port: u16,
    pub ip_version: IpVersion,
    /// Kernel socket inode. This is the *primary key* that lets us join a
    /// socket to a process/unit without relying on `ss -p`.
    pub inode: Option<u64>,
    /// Socket UID, available via `ss -e` WITHOUT process privileges.
    pub uid: Option<u32>,
    /// cgroup v2 path from `ss -e`, available WITHOUT process privileges.
    /// This is the cheapest reliable unit association.
    pub cgroup: Option<String>,
}

impl Endpoint {
    /// Stable identity used across refreshes.
    pub fn key(&self) -> EndpointKey {
        EndpointKey {
            protocol: self.protocol,
            addr: self.local_addr.clone(),
            port: self.local_port,
        }
    }

    pub fn display_addr(&self) -> String {
        match &self.scope {
            Some(s) => format!("{}%{}", self.local_addr, s),
            None => self.local_addr.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EndpointKey {
    pub protocol: Protocol,
    pub addr: String,
    pub port: u16,
}

// ---------------------------------------------------------------------------
// ProcessInfo
// ---------------------------------------------------------------------------

/// How we obtained a PID. Recorded because availability differs by privilege.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PidSource {
    /// `ss -p` — only works for processes owned by the caller.
    SsProcessField,
    /// Read from `/sys/fs/cgroup/<unit>/cgroup.procs` — works unprivileged.
    CgroupProcs,
    /// systemd D-Bus `MainPID` — works unprivileged for system units.
    SystemdMainPid,
    /// The test harness's own PID (controlled process).
    TestHarness,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessInfo {
    pub pid: u32,
    /// `comm` (short name, usually <= 15 chars).
    pub name: String,
    /// argv as read from `/proc/<pid>/cmdline`. `None` when unreadable.
    pub cmdline: Option<Vec<String>>,
    pub uid: Option<u32>,
    pub user: Option<String>,
    pub ppid: Option<u32>,
    /// starttime in clock ticks since boot — the anti-PID-reuse guard.
    pub start_time_ticks: Option<u64>,
    pub source: PidSource,
}

impl ProcessInfo {
    pub fn cmdline_display(&self) -> String {
        match &self.cmdline {
            Some(parts) if !parts.is_empty() => parts.join(" "),
            Some(_) => String::new(),
            None => String::new(),
        }
    }
}

/// Identity that survives PID reuse.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProcessIdentity {
    pub pid: u32,
    /// `/proc/<pid>/stat` field 22. Two different processes reusing a PID
    /// will (in practice) differ here.
    pub start_time_ticks: u64,
}

// ---------------------------------------------------------------------------
// ServiceInfo
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum UnitScope {
    System,
    User,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnitKind {
    Service,
    Socket,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActiveState {
    Active,
    Inactive,
    Failed,
    Activating,
    Deactivating,
    Reloading,
    /// Unit not found / not loadable.
    NotFound,
    /// Could not determine (D-Bus/CLI failure).
    Unknown,
}

impl ActiveState {
    pub fn from_str_systemd(s: &str) -> ActiveState {
        match s {
            "active" => ActiveState::Active,
            "inactive" => ActiveState::Inactive,
            "failed" => ActiveState::Failed,
            "activating" => ActiveState::Activating,
            "deactivating" => ActiveState::Deactivating,
            "reloading" => ActiveState::Reloading,
            _ => ActiveState::Unknown,
        }
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            ActiveState::Active => "active",
            ActiveState::Inactive => "inactive",
            ActiveState::Failed => "failed",
            ActiveState::Activating => "activating",
            ActiveState::Deactivating => "deactivating",
            ActiveState::Reloading => "reloading",
            ActiveState::NotFound => "not-found",
            ActiveState::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceInfo {
    /// Full unit name, e.g. `ssh.service`, `ssh.socket`.
    pub id: String,
    pub scope: UnitScope,
    pub kind: UnitKind,
    pub active_state: ActiveState,
    pub sub_state: String,
    pub load_state: String,
    pub main_pid: Option<u32>,
    /// `ControlGroup` property of the unit, e.g.
    /// `/user.slice/user-1000.slice/user@1000.service/app.slice/psprobe.socket`
    /// or `/system.slice/ssh.socket`. For user-manager socket activation this
    /// is the ONLY reliable link from a socket to its unit, because the fd is
    /// held by `systemd --user` and shows up in `init.scope`.
    pub control_group: Option<String>,
    /// For socket units: `Listen` properties (e.g. `0.0.0.0:22 (Stream)`).
    pub listen: Vec<String>,
    /// Unit(s) this socket activates (`Triggers`).
    pub triggers: Vec<String>,
    /// Socket unit that activates this service (`TriggeredBy`).
    pub triggered_by: Vec<String>,
    /// Path of the unit file, when known.
    pub fragment_path: Option<String>,
    /// How this unit was discovered.
    pub source: ServiceSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServiceSource {
    Systemctl,
    DBus,
    CgroupPath,
}

// ---------------------------------------------------------------------------
// JournalEntry
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalEntry {
    /// Real-time timestamp (microseconds since epoch), sourced from
    /// `__REALTIME_TIMESTAMP` so it is not locale-formatted.
    pub timestamp_usec: u64,
    /// syslog priority 0..7 (0=emerg .. 7=debug). `None` if absent.
    pub priority: Option<u8>,
    /// Raw message. NEVER translated.
    pub message: String,
    pub identifier: Option<String>,
    pub pid: Option<u32>,
    pub unit: Option<String>,
}

impl JournalEntry {
    pub fn priority_label(&self) -> &'static str {
        match self.priority {
            Some(0) => "emerg",
            Some(1) => "alert",
            Some(2) => "crit",
            Some(3) => "err",
            Some(4) => "warning",
            Some(5) => "notice",
            Some(6) => "info",
            Some(7) => "debug",
            Some(_) => "unknown",
            None => "unknown",
        }
    }
}

// ---------------------------------------------------------------------------
// Evidence / Association
// ---------------------------------------------------------------------------

/// The *kind* of proof behind a link. Ordered loosely by strength.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EvidenceKind {
    /// `ss -p` reported this PID for this socket. Strong.
    SocketProcessField,
    /// Socket inode appears in `/proc/<pid>/fd`, or PID's cgroup matches the
    /// socket's cgroup and the socket is bound by that process. Strong.
    SocketInodeInProcFd,
    /// Socket cgroup path equals the cgroup of the unit (`/system.slice/x.service`).
    /// Strong for "unit owns socket", even when PID is unknown.
    CgroupUnitMatch,
    /// systemd socket unit `Triggers=` names the service, and the service is
    /// the one systemd records as activated by that socket.
    SocketActivation,
    /// Unit `MainPID` equals the socket-owning PID.
    MainPidMatch,
    /// ONLY in tests: the harness knows it created the process.
    HarnessInjected,
}

impl EvidenceKind {
    /// Confidence, used only for choosing which evidence to *display* first.
    /// Not a statistical claim.
    pub fn strength(&self) -> u8 {
        match self {
            EvidenceKind::SocketInodeInProcFd => 5,
            EvidenceKind::SocketProcessField => 4,
            EvidenceKind::MainPidMatch => 4,
            EvidenceKind::CgroupUnitMatch => 3,
            EvidenceKind::SocketActivation => 3,
            EvidenceKind::HarnessInjected => 5,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    pub kind: EvidenceKind,
    /// Human-readable, untranslated raw detail, e.g.
    /// `cgroup:/system.slice/ssh.socket` or `MainPID=3576050`.
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AssociationKind {
    EndpointToProcess {
        endpoint: EndpointKey,
        identity: ProcessIdentity,
    },
    EndpointToService {
        endpoint: EndpointKey,
        unit: String,
        scope: UnitScope,
    },
    ServiceToProcess {
        unit: String,
        identity: ProcessIdentity,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Association {
    pub kind: AssociationKind,
    pub evidence: Vec<Evidence>,
    /// Set when evidence is conclusive for *ownership* vs merely indicative.
    pub confident: bool,
}

impl Association {
    pub fn new(kind: AssociationKind, evidence: Vec<Evidence>) -> Self {
        let confident = evidence.iter().any(|e| e.kind.strength() >= 3);
        Association {
            kind,
            evidence,
            confident,
        }
    }
}

// ---------------------------------------------------------------------------
// CollectionError
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CollectionError {
    /// The required binary is absent.
    ToolMissing { tool: String },
    /// Binary exists but the subcommand/flag is unsupported (e.g. `ss --json`
    /// on iproute2 6.1).
    ToolUnsupported { tool: String, feature: String },
    /// The tool failed; `stderr` is kept verbatim for the report.
    ToolFailed {
        tool: String,
        exit: i32,
        stderr: String,
    },
    /// The tool did not finish within the deadline and was killed.
    /// Added in Phase 1 (audit P0-D1): the Spike had no timeout at all.
    Timeout { tool: String, millis: u64 },
    /// The caller cancelled the operation (e.g. a superseded UI refresh).
    Cancelled { tool: String },
    /// The tool produced more output than the configured cap; it was killed
    /// rather than buffered without bound.
    OutputTooLarge { tool: String, limit: usize },
    /// Output could not be parsed. Contains the offending line.
    ParseError {
        context: String,
        line: String,
        reason: String,
    },
    /// Insufficient permissions. Distinguishes "denied" from "empty".
    PermissionDenied { resource: String, hint: String },
    /// Process vanished between listing and inspection.
    ProcessGone { pid: u32 },
    /// Process exists but is a different one than we saw (PID reuse).
    ProcessIdentityChanged {
        pid: u32,
        expected_ticks: u64,
        actual_ticks: u64,
    },
    /// systemd unit does not exist.
    UnitNotFound { unit: String },
    /// journald reachable but no entries in the requested window.
    NoJournalEntries { unit: Option<String> },
    /// journald explicitly reported it would not open files for us.
    JournalPermissionDenied { hint: String },
    /// We wrote a query whose result was truncated by our own limit.
    Truncated { limit: usize },
    /// I/O failure reading /proc or /sys.
    Io { path: String, message: String },
}

impl CollectionError {
    /// A stable machine key, for grouping in the UI and tests. Not translated.
    pub fn code(&self) -> &'static str {
        match self {
            CollectionError::ToolMissing { .. } => "tool_missing",
            CollectionError::ToolUnsupported { .. } => "tool_unsupported",
            CollectionError::ToolFailed { .. } => "tool_failed",
            CollectionError::Timeout { .. } => "timeout",
            CollectionError::Cancelled { .. } => "cancelled",
            CollectionError::OutputTooLarge { .. } => "output_too_large",
            CollectionError::ParseError { .. } => "parse_error",
            CollectionError::PermissionDenied { .. } => "permission_denied",
            CollectionError::ProcessGone { .. } => "process_gone",
            CollectionError::ProcessIdentityChanged { .. } => "pid_reused",
            CollectionError::UnitNotFound { .. } => "unit_not_found",
            CollectionError::NoJournalEntries { .. } => "no_journal_entries",
            CollectionError::JournalPermissionDenied { .. } => "journal_permission_denied",
            CollectionError::Truncated { .. } => "truncated",
            CollectionError::Io { .. } => "io_error",
        }
    }
}

impl fmt::Display for CollectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CollectionError::ToolMissing { tool } => write!(f, "required tool missing: {tool}"),
            CollectionError::ToolUnsupported { tool, feature } => {
                write!(f, "{tool} does not support {feature}")
            }
            CollectionError::ToolFailed { tool, exit, stderr } => {
                write!(f, "{tool} exited {exit}: {}", stderr.trim())
            }
            CollectionError::Timeout { tool, millis } => {
                write!(f, "{tool} timed out after {:.1}s", *millis as f64 / 1000.0)
            }
            CollectionError::Cancelled { tool } => write!(f, "{tool} was cancelled"),
            CollectionError::OutputTooLarge { tool, limit } => {
                write!(f, "{tool} produced more than {limit} bytes of output")
            }
            CollectionError::ParseError {
                context,
                line,
                reason,
            } => {
                write!(f, "parse error in {context}: {reason} (line: {line})")
            }
            CollectionError::PermissionDenied { resource, hint } => {
                write!(f, "permission denied for {resource}: {hint}")
            }
            CollectionError::ProcessGone { pid } => write!(f, "process {pid} exited"),
            CollectionError::ProcessIdentityChanged { pid, .. } => {
                write!(f, "pid {pid} was reused by a different process")
            }
            CollectionError::UnitNotFound { unit } => write!(f, "unit not found: {unit}"),
            CollectionError::NoJournalEntries { unit } => match unit {
                Some(u) => write!(f, "no journal entries for {u}"),
                None => write!(f, "no journal entries"),
            },
            CollectionError::JournalPermissionDenied { hint } => {
                write!(f, "journal not readable: {hint}")
            }
            CollectionError::Truncated { limit } => write!(f, "result truncated at {limit} items"),
            CollectionError::Io { path, message } => write!(f, "{path}: {message}"),
        }
    }
}

impl std::error::Error for CollectionError {}

// ---------------------------------------------------------------------------
// Snapshot
// ---------------------------------------------------------------------------

/// One collection cycle. Partial failures are carried alongside the data.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Snapshot {
    pub collected_at: Option<SystemTime>,
    pub endpoints: Vec<Endpoint>,
    pub processes: Vec<ProcessInfo>,
    pub services: Vec<ServiceInfo>,
    pub associations: Vec<Association>,
    pub errors: Vec<CollectionError>,
    /// True when a collector degraded (e.g. no process info available).
    pub degraded: bool,
}

impl Snapshot {
    pub fn push_error(&mut self, e: CollectionError) {
        self.errors.push(e);
    }

    pub fn process_by_identity(&self, id: &ProcessIdentity) -> Option<&ProcessInfo> {
        self.processes
            .iter()
            .find(|p| p.pid == id.pid && p.start_time_ticks == Some(id.start_time_ticks))
    }

    pub fn service_by_id(&self, id: &str) -> Option<&ServiceInfo> {
        self.services.iter().find(|s| s.id == id)
    }
}
