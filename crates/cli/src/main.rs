//! PortScope diagnostic CLI.
//!
//! Purpose: give the collector a reproducible, scriptable front end so that
//! tests and evidence logs do not need a display server. Subcommands mirror the
//! Phase 1 acceptance work:
//!
//! ```text
//! portscope-cli snapshot [--json PATH] [--timeout SECS]
//! portscope-cli journal [UNIT] [system|user]
//! portscope-cli compare-backends
//! ```

use portscope_core::collect::{CollectOptions, collect_cancellable};
use portscope_core::exec::Cancel;
use portscope_core::journal::{JournalOutcome, JournalQuery, query_journal};
use portscope_core::model::*;
use portscope_core::services::{Backend, collect_services};
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(String::as_str).unwrap_or("snapshot");
    let code = match cmd {
        "snapshot" => snapshot(&args[2..]),
        "journal" => journal(args.get(2).cloned(), args.get(3).cloned()),
        "compare-backends" => compare_backends(),
        "-h" | "--help" | "help" => {
            println!(
                "usage: portscope-cli <command>\n\
                 \n\
                 commands:\n\
                 \x20 snapshot [--json PATH]   collect and print a diagnostic snapshot\n\
                 \x20 journal [UNIT] [system|user]\n\
                 \x20 compare-backends         cross-check D-Bus against systemctl\n"
            );
            0
        }
        other => {
            eprintln!("unknown command: {other} (try --help)");
            2
        }
    };
    std::process::exit(code);
}

fn snapshot(args: &[String]) -> i32 {
    let mut json_path: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--json" => {
                json_path = args.get(i + 1).cloned();
                i += 2;
            }
            other => {
                eprintln!("snapshot: unknown argument {other}");
                return 2;
            }
        }
    }

    // A CLI run is not interactive: a generous but finite overall budget.
    let cancel = Cancel::new();
    let t0 = Instant::now();
    let snap = collect_cancellable(&CollectOptions::default(), &cancel);
    let elapsed = t0.elapsed();

    println!("== PortScope snapshot ==");
    println!("collected_at = {:?}", snap.collected_at);
    println!("duration_ms  = {:.1}", elapsed.as_secs_f64() * 1000.0);
    println!("endpoints    = {}", snap.endpoints.len());
    println!("processes    = {}", snap.processes.len());
    println!("services     = {}", snap.services.len());
    println!("associations = {}", snap.associations.len());
    println!("degraded     = {}", snap.degraded);
    println!();

    println!("== endpoints (first 25) ==");
    println!(
        "{:<4} {:<7} {:<28} {:>6} {:>8} {:<10} UNIT(cgroup-derived)",
        "PROTO", "STATE", "LOCAL", "PORT", "UID", "PID"
    );
    for ep in snap.endpoints.iter().take(25) {
        let pid = snap
            .associations
            .iter()
            .find_map(|a| match &a.kind {
                AssociationKind::EndpointToProcess { endpoint, identity }
                    if *endpoint == ep.key() =>
                {
                    Some(identity.pid.to_string())
                }
                _ => None,
            })
            .unwrap_or_else(|| "-".into());
        let unit = snap
            .associations
            .iter()
            .find_map(|a| match &a.kind {
                AssociationKind::EndpointToService { endpoint, unit, .. }
                    if *endpoint == ep.key() =>
                {
                    Some(unit.clone())
                }
                _ => None,
            })
            .unwrap_or_else(|| "-".into());
        println!(
            "{:<4} {:<7} {:<28} {:>6} {:>8} {:<10} {}",
            ep.protocol.as_str(),
            ep.state.label(),
            ep.display_addr(),
            ep.local_port,
            ep.uid.map(|u| u.to_string()).unwrap_or_else(|| "-".into()),
            pid,
            unit
        );
    }
    if snap.endpoints.len() > 25 {
        println!("... {} more", snap.endpoints.len() - 25);
    }

    println!("\n== association evidence samples ==");
    for a in snap.associations.iter().take(10) {
        let kinds: Vec<String> = a
            .evidence
            .iter()
            .map(|e| format!("{:?}({})", e.kind, e.detail))
            .collect();
        println!(
            "{:?}\n   confident={} evidence={}",
            a.kind,
            a.confident,
            kinds.join(" | ")
        );
    }

    println!("\n== errors ({}) ==", snap.errors.len());
    for e in &snap.errors {
        println!("[{:<28}] {}", e.code(), e);
    }

    if let Some(path) = json_path {
        match serde_json::to_string_pretty(&snap) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&path, json) {
                    eprintln!("cannot write {path}: {e}");
                    return 1;
                }
                println!("\njson written to {path}");
            }
            Err(e) => {
                eprintln!("cannot serialise snapshot: {e}");
                return 1;
            }
        }
    }
    0
}

fn journal(unit: Option<String>, scope: Option<String>) -> i32 {
    let scope = match scope.as_deref() {
        Some("user") => UnitScope::User,
        _ => UnitScope::System,
    };
    let q = JournalQuery {
        scope,
        unit: unit.clone(),
        since_secs: 86_400,
        max_entries: 20,
    };
    let t0 = Instant::now();
    let res = query_journal(&q);
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    println!(
        "== journal query unit={:?} scope={:?} ({:.1} ms) ==",
        unit, scope, ms
    );
    match res {
        Ok(JournalOutcome::Entries(e)) => {
            println!("entries = {}", e.len());
            for en in e.iter().take(5) {
                println!(
                    "  ts={} prio={}({}) id={:?} pid={:?} msg={}",
                    en.timestamp_usec,
                    en.priority
                        .map(|p| p.to_string())
                        .unwrap_or_else(|| "-".into()),
                    en.priority_label(),
                    en.identifier,
                    en.pid,
                    en.message.chars().take(100).collect::<String>()
                );
            }
            0
        }
        Ok(JournalOutcome::Empty) => {
            println!("EMPTY: journal reachable, no entries in window");
            0
        }
        Ok(JournalOutcome::Denied(h)) => {
            println!("DENIED: {h}");
            0
        }
        Err(e) => {
            println!("ERROR [{}]: {}", e.code(), e);
            1
        }
    }
}

fn compare_backends() -> i32 {
    println!("== systemctl vs D-Bus: unit state agreement & cost ==");
    let t0 = Instant::now();
    let (cli_units, _e) = collect_services(&[UnitScope::System], None, Backend::Systemctl);
    let cli_ms = t0.elapsed().as_secs_f64() * 1000.0;

    let names: Vec<String> = cli_units.iter().take(40).map(|s| s.id.clone()).collect();
    let t1 = Instant::now();
    let (dbus, derrs) = collect_services(&[UnitScope::System], Some(&names), Backend::DBus);
    let dbus_ms = t1.elapsed().as_secs_f64() * 1000.0;

    println!("systemctl: {} units in {:.1} ms", cli_units.len(), cli_ms);
    println!(
        "dbus     : {} units in {:.1} ms ({} errors)",
        dbus.len(),
        dbus_ms,
        derrs.len()
    );

    let mut agree = 0;
    let mut disagree = 0;
    for d in &dbus {
        if let Some(c) = cli_units.iter().find(|c| c.id == d.id) {
            if c.active_state == d.active_state {
                agree += 1;
            } else {
                disagree += 1;
                println!(
                    "  DISAGREE {} systemctl={} dbus={}",
                    d.id,
                    c.active_state.as_str(),
                    d.active_state.as_str()
                );
            }
        }
    }
    println!("active-state agreement: {agree} agree / {disagree} disagree");
    println!(
        "dbus errors: {:?}",
        derrs.iter().map(|e| e.code()).collect::<Vec<_>>()
    );
    if disagree == 0 { 0 } else { 1 }
}
