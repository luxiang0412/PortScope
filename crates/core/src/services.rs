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
}
