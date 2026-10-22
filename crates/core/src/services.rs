//! systemd unit collection.
//!
//! Two backends are implemented so the spike can compare them on real data:
//!   * `Systemctl` — shells out to `systemctl show` (batch via many `-p`).
//!   * `DBus`      — calls `busctl get-property` on org.freedesktop.systemd1.
//!
//! Both are unprivileged. The D-Bus path is measured in tests; the shell path
//! is the default because it is trivially debuggable and needs no session bus
//! ownership assumptions.

use crate::endpoints::run_capture;
use crate::model::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Systemctl,
    DBus,
}

fn scope_args(scope: UnitScope) -> Vec<&'static str> {
    match scope {
        UnitScope::System => vec![],
        UnitScope::User => vec!["--user"],
    }
}

/// List unit names of a given type.
fn list_units(scope: UnitScope, kind: UnitKind) -> Result<Vec<String>, CollectionError> {
    let t = match kind {
        UnitKind::Service => "--type=service",
        UnitKind::Socket => "--type=socket",
        UnitKind::Unknown => return Ok(vec![]),
    };
    let mut args = scope_args(scope);
    args.extend_from_slice(&[
        "list-units",
        t,
        "--all",
        "--no-pager",
        "--no-legend",
        "--plain",
    ]);

    let tool = "systemctl";
    let out = run_capture(tool, &args).map_err(|e| match e {
        CollectionError::ToolFailed { stderr, .. }
            if stderr.contains("Failed to connect to bus")
                || stderr.contains("No medium found")
                || stderr.contains("not been booted with systemd") =>
        {
            CollectionError::PermissionDenied {
                resource: format!("systemctl {:?}", scope),
                hint: "user/system bus unavailable (no systemd session?)".into(),
            }
        }
        other => other,
    })?;

    // Each line: "NAME LOAD ACTIVE SUB DESCRIPTION"
    Ok(out
        .lines()
        .filter_map(|l| {
            let name = l.split_whitespace().next()?;
            if name.ends_with(".service") || name.ends_with(".socket") {
                Some(name.to_string())
            } else {
                None
            }
        })
        .collect())
}

/// Properties we need, in one batch call per unit.
const WANTED_PROPS: &[&str] = &[
    "Id",
    "ActiveState",
    "SubState",
    "LoadState",
    "MainPID",
    "Listen",
    "Triggers",
    "TriggeredBy",
    "FragmentPath",
    "ControlGroup",
];

fn show_unit_systemctl(scope: UnitScope, unit: &str) -> Result<ServiceInfo, CollectionError> {
    let mut args = scope_args(scope);
    args.push("show");
    args.push(unit);
    for p in WANTED_PROPS {
        args.push("-p");
        args.push(p);
    }
    args.push("--no-pager");
    let out = run_capture("systemctl", &args)?;

    let mut map = std::collections::HashMap::new();
    for line in out.lines() {
        if let Some((k, v)) = line.split_once('=') {
            map.entry(k.to_string()).or_insert_with(|| v.to_string());
        } else if !line.trim().is_empty() {
            // systemd repeats multi-value props on separate lines with same key;
            // handled below for Listen/Triggers/TriggeredBy.
            continue;
        }
    }
    // Multi-value properties must be gathered across all lines.
    let collect_multi = |key: &str| -> Vec<String> {
        out.lines()
            .filter_map(|l| l.strip_prefix(&format!("{}=", key)))
            .filter(|v| !v.is_empty())
            .map(|v| v.to_string())
            .collect()
    };

    let load_state = map.get("LoadState").cloned().unwrap_or_default();
    if load_state == "not-found" {
        return Err(CollectionError::UnitNotFound { unit: unit.into() });
    }

    let id = map.get("Id").cloned().unwrap_or_else(|| unit.to_string());
    if id.is_empty() {
        return Err(CollectionError::UnitNotFound { unit: unit.into() });
    }
    let main_pid = map
        .get("MainPID")
        .and_then(|v| v.parse::<u32>().ok())
        .filter(|v| *v != 0);

    Ok(ServiceInfo {
        kind: if id.ends_with(".socket") {
            UnitKind::Socket
        } else if id.ends_with(".service") {
            UnitKind::Service
        } else {
            UnitKind::Unknown
        },
        active_state: map
            .get("ActiveState")
            .map(|s| ActiveState::from_str_systemd(s))
            .unwrap_or(ActiveState::Unknown),
        sub_state: map.get("SubState").cloned().unwrap_or_default(),
        load_state,
        main_pid,
        listen: collect_multi("Listen"),
        triggers: collect_multi("Triggers"),
        triggered_by: collect_multi("TriggeredBy"),
        fragment_path: map.get("FragmentPath").filter(|s| !s.is_empty()).cloned(),
        control_group: map.get("ControlGroup").filter(|s| !s.is_empty()).cloned(),
        source: ServiceSource::Systemctl,
        scope,
        id,
    })
}

/// D-Bus variant, for comparison. Uses `busctl` (part of systemd, present on
/// Ubuntu/Debian) rather than a Rust D-Bus crate to keep dependencies small.
///
/// NOTE: `busctl call ... GetAll` prints one long line of GVariant text, e.g.
///   a{sv} 99 "ActiveState" s "active" "SubState" s "running" ...
/// so a naive `splitn(3, ' ')` on lines does NOT work (this was a real bug
/// caught by comparing against `systemctl`; see SPIKE-REPORT.md).
fn show_unit_dbus(scope: UnitScope, unit: &str) -> Result<ServiceInfo, CollectionError> {
    let bus_flag = match scope {
        UnitScope::System => "--system",
        UnitScope::User => "--user",
    };
    let path = format!(
        "/org/freedesktop/systemd1/unit/{}",
        escape_dbus_unit_name(unit)
    );
    let props = busctl_get_all(bus_flag, &path, "org.freedesktop.systemd1.Unit")?;

    let active = props
        .get("ActiveState")
        .map(|v| v.trim_matches('"').to_string())
        .unwrap_or_default();
    if active.is_empty() {
        return Err(CollectionError::UnitNotFound { unit: unit.into() });
    }
    // D-Bus represents a missing unit as a stub object with
    // LoadState=not-found and ActiveState=inactive (rather than an error).
    // Normalise this to the same error the systemctl backend produces so
    // callers do not have to special-case backends.
    if props.get("LoadState").map(|v| v.trim_matches('"')) == Some("not-found") {
        return Err(CollectionError::UnitNotFound { unit: unit.into() });
    }
    // Service-specific properties live on the Service interface, not Unit.
    let svc_props = if unit.ends_with(".service") {
        busctl_get_all(bus_flag, &path, "org.freedesktop.systemd1.Service").unwrap_or_default()
    } else if unit.ends_with(".socket") {
        busctl_get_all(bus_flag, &path, "org.freedesktop.systemd1.Socket").unwrap_or_default()
    } else {
        Default::default()
    };

    let s = |map: &std::collections::HashMap<String, String>, k: &str| -> Option<String> {
        map.get(k).map(|v| v.trim_matches('"').to_string())
    };

    Ok(ServiceInfo {
        id: s(&props, "Id").unwrap_or_else(|| unit.to_string()),
        scope,
        kind: if unit.ends_with(".socket") {
            UnitKind::Socket
        } else if unit.ends_with(".service") {
            UnitKind::Service
        } else {
            UnitKind::Unknown
        },
        active_state: ActiveState::from_str_systemd(&active),
        sub_state: s(&props, "SubState").unwrap_or_default(),
        load_state: s(&props, "LoadState").unwrap_or_default(),
        main_pid: s(&svc_props, "MainPID")
            .and_then(|v| v.split_whitespace().last().map(str::to_string))
            .and_then(|v| v.parse::<u32>().ok())
            .filter(|v| *v != 0),
        // ControlGroup is NOT on the Unit interface; it is on Service/Socket.
        control_group: s(&svc_props, "ControlGroup").filter(|v| !v.is_empty()),
        // Enumerating arrays/structs from GVariant text is out of scope here;
        // the systemctl backend is authoritative for Listen=/Triggers=.
        listen: vec![],
        triggers: vec![],
        triggered_by: vec![],
        fragment_path: s(&props, "FragmentPath").filter(|v| !v.is_empty()),
        source: ServiceSource::DBus,
    })
}

/// Fetch all properties of one interface as `name -> raw GVariant value`.
fn busctl_get_all(
    bus_flag: &str,
    object_path: &str,
    iface: &str,
) -> Result<std::collections::HashMap<String, String>, CollectionError> {
    let out = run_capture(
        "busctl",
        &[
            bus_flag,
            "call",
            "org.freedesktop.systemd1",
            object_path,
            "org.freedesktop.DBus.Properties",
            "GetAll",
            "s",
            iface,
        ],
    )?;
    Ok(parse_gvariant_dict(&out))
}

/// Parse the first key/value pair of each top-level `"key" <type> <value>` run.
///
/// This handles the simple scalar types we need (s, b, t, u, i) and ignores
/// arrays/structs. It is intentionally conservative: unknown shapes are
/// skipped rather than mis-parsed.
fn parse_gvariant_dict(text: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let tokens = tokenize_gvariant(text);
    let mut i = 0;
    while i + 1 < tokens.len() {
        let key = &tokens[i];
        if key.starts_with('"') && is_simple_type(&tokens[i + 1]) {
            // Store the VALUE only. Keeping the GVariant type token here was a
            // real bug: `Id` became `s "ssh.service"` and every state lookup
            // returned Unknown.
            if i + 2 < tokens.len() {
                let val = &tokens[i + 2];
                map.insert(key.trim_matches('"').to_string(), val.clone());
                i += 3;
                continue;
            }
        }
        i += 1;
    }
    map
}

fn is_simple_type(t: &str) -> bool {
    matches!(t, "s" | "b" | "t" | "u" | "i" | "x" | "d" | "o")
}

/// Split GVariant text into tokens, keeping quoted strings intact.
fn tokenize_gvariant(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut escaped = false;
    for ch in text.chars() {
        if in_quotes {
            cur.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_quotes = false;
                out.push(std::mem::take(&mut cur));
            }
            continue;
        }
        match ch {
            '"' => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                in_quotes = true;
                cur.push(ch);
            }
            c if c.is_whitespace() => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// systemd escapes non-alphanumerics in D-Bus object paths as `_XX`.
pub fn escape_dbus_unit_name(unit: &str) -> String {
    let mut out = String::with_capacity(unit.len());
    for ch in unit.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else {
            out.push_str(&format!("_{:02x}", ch as u32));
        }
    }
    out
}

pub fn show_unit(
    scope: UnitScope,
    unit: &str,
    backend: Backend,
) -> Result<ServiceInfo, CollectionError> {
    match backend {
        Backend::Systemctl => show_unit_systemctl(scope, unit),
        Backend::DBus => show_unit_dbus(scope, unit),
    }
}

/// Collect all units for the given scopes and kinds.
/// Partial failures are returned alongside successes.
pub fn collect_services(
    scopes: &[UnitScope],
    units: Option<&[String]>,
    backend: Backend,
) -> (Vec<ServiceInfo>, Vec<CollectionError>) {
    let mut out = Vec::new();
    let mut errors = Vec::new();
    for &scope in scopes {
        let names: Vec<String> = match units {
            Some(u) => u.to_vec(),
            None => match list_units(scope, UnitKind::Service) {
                Ok(mut a) => {
                    match list_units(scope, UnitKind::Socket) {
                        Ok(b) => a.extend(b),
                        Err(e) => errors.push(e),
                    }
                    a
                }
                Err(e) => {
                    errors.push(e);
                    continue;
                }
            },
        };
        for n in names {
            match show_unit(scope, &n, backend) {
                Ok(s) => out.push(s),
                Err(CollectionError::UnitNotFound { .. }) => {
                    // Listing can race with unit teardown; not fatal.
                }
                Err(e) => errors.push(e),
            }
        }
    }
    (out, errors)
}

// ---------------------------------------------------------------------------
// Batched D-Bus enumeration (Phase 2, fixes P1-D1)
// ---------------------------------------------------------------------------
//
// The Phase 1 D-Bus path called `busctl` once per unit per interface. Measured
// cost was ~27 ms per spawn, so a full snapshot over ~305 units took 7-8 s —
// too slow for the 5 s auto-refresh the UI spec requires.
//
// `Manager.ListUnits` returns name, description, load/active/sub state and the
// object path for EVERY unit in ONE call (~8 ms for 704 units here). That is
// enough to populate the Services view without any per-unit traffic.
//
// Per-unit `GetAll` is then needed only for the rarely-required extras
// (MainPID, ControlGroup, Listen=, Triggers=). Those are fetched lazily, for
// the units the user actually looks at, not for all 300 on every refresh.

/// One row from `Manager.ListUnits`, before per-unit detail is loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitListing {
    pub name: String,
    pub description: String,
    pub load_state: String,
    pub active_state: ActiveState,
    pub sub_state: String,
    pub object_path: String,
}

impl UnitListing {
    /// Convert to a `ServiceInfo` with the detail-only fields absent.
    ///
    /// `main_pid`, `control_group`, `listen` and `triggers` are `None`/empty
    /// here on purpose: the Services *list* does not need them, and claiming
    /// otherwise would misrepresent an unqueried field as a known-empty one.
    pub fn to_service_info(&self, scope: UnitScope) -> ServiceInfo {
        ServiceInfo {
            id: self.name.clone(),
            scope,
            kind: if self.name.ends_with(".socket") {
                UnitKind::Socket
            } else if self.name.ends_with(".service") {
                UnitKind::Service
            } else {
                UnitKind::Unknown
            },
            active_state: self.active_state,
            sub_state: self.sub_state.clone(),
            load_state: self.load_state.clone(),
            main_pid: None,
            control_group: None,
            listen: vec![],
            triggers: vec![],
            triggered_by: vec![],
            fragment_path: None,
            source: ServiceSource::DBus,
        }
    }
}

/// Decode GVariant `\xNN` escapes used for non-alphanumeric characters.
///
/// `busctl` prints unit names such as `snap-snap\x2dstore-1447.mount`, so a
/// naive read yields names that do not match `systemctl` or `/proc/<pid>/cgroup`.
fn unescape_gvariant(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            match bytes[i + 1] {
                b'x' if i + 3 < bytes.len() => {
                    let hex = &s[i + 2..i + 4];
                    if let Ok(v) = u8::from_str_radix(hex, 16) {
                        out.push(v as char);
                        i += 4;
                        continue;
                    }
                }
                // busctl also emits \\ for a literal backslash.
                b'\\' => {
                    out.push('\\');
                    i += 2;
                    continue;
                }
                _ => {}
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

/// Parse the `a(ssssssouso)` output of `Manager.ListUnits`.
///
/// We take the tokens in groups of 10 per struct. Only the fields we need are
/// kept; malformed trailing data is ignored rather than failing the whole call.
fn parse_list_units(text: &str) -> Vec<UnitListing> {
    // Tokens as produced by the existing conservative tokenizer (quoted strings
    // stay as single tokens including their quotes).
    let tokens = tokenize_gvariant(text);
    let mut out = Vec::new();
    // Skip the leading signature token `a(ssssssouso)` and the count.
    let mut i = 0;
    while i < tokens.len() {
        if tokens[i].starts_with('"') {
            break;
        }
        i += 1;
    }
    // Each struct contributes 10 tokens. The count token is not quoted, so
    // groups are aligned from the first quoted token.
    while i + 9 < tokens.len() {
        let name = tokens[i].trim_matches('"');
        let desc = tokens[i + 1].trim_matches('"');
        let load = tokens[i + 2].trim_matches('"');
        let active = tokens[i + 3].trim_matches('"');
        let sub = tokens[i + 4].trim_matches('"');
        // i+5 following, i+6 object path, i+7 job id, i+8 job type, i+9 job path
        let path = tokens[i + 6].trim_matches('"');
        if !name.is_empty() && path.starts_with('/') {
            out.push(UnitListing {
                name: unescape_gvariant(name),
                description: unescape_gvariant(desc),
                load_state: load.to_string(),
                active_state: ActiveState::from_str_systemd(active),
                sub_state: sub.to_string(),
                object_path: path.to_string(),
            });
        }
        i += 10;
    }
    out
}

/// Enumerate units with a single `ListUnits` call.
///
/// `kinds` filters to service/socket units. Session plumbing
/// (`init.scope`, `session-*.scope`) is excluded because it is where a process
/// runs, not a service (taskbook §4.5).
pub fn list_units_batched(
    scope: UnitScope,
    kinds: &[UnitKind],
) -> Result<Vec<UnitListing>, CollectionError> {
    let bus_flag = match scope {
        UnitScope::System => "--system",
        UnitScope::User => "--user",
    };
    let cancel = crate::exec::Cancel::new();
    let out = crate::exec::run_checked(
        "busctl",
        &[
            bus_flag,
            "call",
            "org.freedesktop.systemd1",
            "/org/freedesktop/systemd1",
            "org.freedesktop.systemd1.Manager",
            "ListUnits",
        ],
        &crate::exec::ExecLimits::slow(),
        &cancel,
    )
    .map_err(|e| match e {
        CollectionError::ToolFailed { stderr, .. }
            if stderr.contains("Failed to connect to bus")
                || stderr.contains("No medium found")
                || stderr.contains("not been booted with systemd")
                || stderr.contains("No such file or directory") =>
        {
            CollectionError::PermissionDenied {
                resource: format!("session/system bus ({scope:?})"),
                hint: "systemd bus unavailable for this scope".into(),
            }
        }
        other => other,
    })?;

    let mut units: Vec<UnitListing> = parse_list_units(&out)
        .into_iter()
        .filter(|u| {
            kinds.iter().any(|k| match k {
                UnitKind::Service => u.name.ends_with(".service"),
                UnitKind::Socket => u.name.ends_with(".socket"),
                UnitKind::Unknown => false,
            })
        })
        .filter(|u| !is_session_plumbing(&u.name))
        .collect();

    units.sort_by(|a, b| a.name.cmp(&b.name));
    units.dedup_by(|a, b| a.name == b.name);
    Ok(units)
}

/// `init.scope` and `session-*.scope` describe where a process runs, not which
/// unit owns a port. They must not be presented as services (taskbook §4.5).
fn is_session_plumbing(name: &str) -> bool {
    name == "init.scope" || (name.starts_with("session-") && name.ends_with(".scope"))
}

/// Batched collection used by the product path.
///
/// Falls back to the per-unit path only when `ListUnits` is unavailable, so a
/// systemd that cannot enumerate still yields data via `systemctl`.
pub fn collect_services_batched(
    scopes: &[UnitScope],
    backend: Backend,
) -> (Vec<ServiceInfo>, Vec<CollectionError>) {
    let mut out = Vec::new();
    let mut errors = Vec::new();

    for &scope in scopes {
        match backend {
            Backend::DBus => {
                match list_units_batched(scope, &[UnitKind::Service, UnitKind::Socket]) {
                    Ok(listings) => {
                        out.extend(listings.iter().map(|l| l.to_service_info(scope)));
                    }
                    Err(e) => {
                        errors.push(e);
                        // Degrade to systemctl for this scope rather than losing it.
                        let (mut svcs, errs) = collect_services(&[scope], None, Backend::Systemctl);
                        out.append(&mut svcs);
                        errors.extend(errs);
                    }
                }
            }
            Backend::Systemctl => {
                let (mut svcs, errs) = collect_services(&[scope], None, Backend::Systemctl);
                out.append(&mut svcs);
                errors.extend(errs);
            }
        }
    }
    (out, errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dbus_escaping_matches_systemd() {
        assert_eq!(escape_dbus_unit_name("ssh.service"), "ssh_2eservice");
        assert_eq!(
            escape_dbus_unit_name("getty@tty1.service"),
            "getty_40tty1_2eservice"
        );
        assert_eq!(
            escape_dbus_unit_name("a-b_c.service"),
            "a_2db_5fc_2eservice"
        );
    }

    #[test]
    fn nonexistent_unit_is_not_found_systemctl() {
        let e = show_unit(
            UnitScope::System,
            "portscope-nope-xyz.service",
            Backend::Systemctl,
        )
        .unwrap_err();
        assert_eq!(e.code(), "unit_not_found");
    }

    #[test]
    fn nonexistent_unit_is_not_found_dbus() {
        let e = show_unit(
            UnitScope::System,
            "portscope-nope-xyz.service",
            Backend::DBus,
        )
        .unwrap_err();
        assert!(
            e.code() == "unit_not_found" || e.code() == "tool_failed",
            "got {e}"
        );
    }

    #[test]
    fn system_scope_lists_some_units() {
        let (svcs, _errs) = collect_services(&[UnitScope::System], None, Backend::Systemctl);
        assert!(!svcs.is_empty(), "expected system units on a systemd host");
        assert!(svcs.iter().any(|s| s.kind == UnitKind::Service));
        assert!(
            svcs.iter().any(|s| s.kind == UnitKind::Socket),
            "expected socket units"
        );
    }

    #[test]
    fn user_scope_socket_units_have_trigger_info_when_active() {
        let (svcs, _e) = collect_services(&[UnitScope::System], None, Backend::Systemctl);
        // ssh.socket is the canonical socket-activation case on Ubuntu.
        if let Some(s) = svcs.iter().find(|s| s.id == "ssh.socket") {
            assert!(
                s.triggers.iter().any(|t| t.ends_with(".service")),
                "ssh.socket should trigger a service, got {:?}",
                s.triggers
            );
        }
    }

    #[test]
    fn gvariant_escapes_are_decoded() {
        // busctl prints non-alphanumerics as \xNN; unit names must match
        // systemctl and /proc cgroups, so these must be decoded.
        assert_eq!(
            unescape_gvariant(r"snap-snap\x2dstore-1447.mount"),
            "snap-snap-store-1447.mount"
        );
        assert_eq!(unescape_gvariant("ssh.service"), "ssh.service");
        assert_eq!(unescape_gvariant(r"a\\b"), r"a\b");
    }

    #[test]
    fn session_plumbing_is_excluded() {
        assert!(is_session_plumbing("init.scope"));
        assert!(is_session_plumbing("session-13.scope"));
        assert!(!is_session_plumbing("ssh.service"));
        assert!(!is_session_plumbing("session-manager.service"));
    }

    #[test]
    fn parse_list_units_reads_struct_fields() {
        let text = r#"a(ssssssouso) 1 "ssh.service" "OpenBSD Secure Shell" "loaded" "active" "running" "" "/org/freedesktop/systemd1/unit/ssh_2eservice" 0 "" "/""#;
        let units = parse_list_units(text);
        assert_eq!(units.len(), 1);
        let u = &units[0];
        assert_eq!(u.name, "ssh.service");
        assert_eq!(u.description, "OpenBSD Secure Shell");
        assert_eq!(u.load_state, "loaded");
        assert_eq!(u.active_state, ActiveState::Active);
        assert_eq!(u.sub_state, "running");
        assert!(u.object_path.starts_with("/org/freedesktop/systemd1/unit/"));
    }

    #[test]
    fn batched_listing_matches_systemctl_count_order_of_magnitude() {
        // Both backends must see the same service/socket universe on a systemd
        // host; a large discrepancy means the ListUnits parser is wrong.
        let batched = list_units_batched(UnitScope::System, &[UnitKind::Service, UnitKind::Socket]);
        let Ok(batched) = batched else {
            return; // no system bus: nothing to compare
        };
        let (cli, _e) = collect_services(&[UnitScope::System], None, Backend::Systemctl);
        assert!(!batched.is_empty());
        for c in cli.iter().filter(|c| c.kind == UnitKind::Service) {
            if is_session_plumbing(&c.id) {
                continue;
            }
            assert!(
                batched.iter().any(|b| b.name == c.id),
                "batched listing is missing {} which systemctl reports",
                c.id
            );
        }
    }

    #[test]
    fn batched_listing_is_much_faster_than_per_unit() {
        use std::time::Instant;
        let t0 = Instant::now();
        let Ok(batched) =
            list_units_batched(UnitScope::System, &[UnitKind::Service, UnitKind::Socket])
        else {
            return;
        };
        let batched_ms = t0.elapsed().as_secs_f64() * 1000.0;
        if batched.len() < 40 {
            return; // not enough units to make a meaningful assertion
        }
        // The per-unit path costs ~27 ms/unit; 40 units alone would be > 1 s.
        // Batched enumeration must stay well under that.
        assert!(
            batched_ms < 1000.0,
            "batched ListUnits took {batched_ms:.0}ms for {} units; expected < 1000ms",
            batched.len()
        );
    }
}
