# PRODUCT — PortScope

> Phase 0 draft. Scope is fixed by
> `PortScope-v1.0-Development-Taskbook.md` §1; anything not built yet is marked
> as such. Do not read this as a feature-complete claim.

## What it is

PortScope is a Linux desktop application for diagnosing **local** ports,
processes, systemd services and journald logs. It answers, for a normal user
without root:

- Which local TCP/UDP endpoints are listening or bound?
- Which process owns this endpoint — and which fields are invisible because of
  permissions?
- Is there a **verifiable** link between this endpoint and a systemd unit?
- What happened recently according to the journal — and is the journal readable
  at all?
- What is the evidence behind each conclusion, and what remains unknown?

## Core workflow (v1.0)

```
discover ports → inspect process → inspect associated service
              → read recent logs → copy diagnostic summary
```

Every step must work in the real GUI. When there is no evidence, the UI says
*unknown*, *unavailable*, or *no confirmable association* — it never guesses from
a name or a port number.

## Scope

In: local TCP/UDP over IPv4/IPv6; process identity (PID, name, UID, PPID, start
time, command line when readable); system and per-user systemd units including
socket activation; bounded recent journald queries; evidence-tracked
associations; search/filter/sort/refresh/details/copy; English and Simplified
Chinese; light and dark themes; adaptive layout, keyboard access; tests,
packaging and OSS release preparation.

Out (v1.0): remote/SSH hosts; network or vulnerability scanning; killing
processes; starting/stopping/editing units; firewall or network changes; running
the GUI as root; automatic group or permission changes; a full log-management
platform; container/Kubernetes management; plugins, sync, telemetry, accounts;
AI features added for their own sake; Electron or WebView shells.

## Non-negotiable product properties

1. **Evidence over inference.** Every endpoint↔service link carries the evidence
   that produced it. No name matching, no port-number guessing.
2. **Honest degradation.** Permission denial, missing tools, vanished processes,
   unreadable journals and stale data are each distinct, visible states.
3. **Unprivileged.** The GUI runs as the invoking user. No root, no sudo/pkexec,
   no group modification.
4. **No network, no telemetry.** Nothing leaves the machine.
5. **Raw data is never mangled.** IPs, ports, PIDs, unit names, cgroup paths and
   journal text are shown verbatim and never translated.

## Status

Nothing in this document is implemented in the production tree yet. See
`docs/STATUS.md` for the authoritative per-phase state and
`evidence/reports/PHASE0-AUDIT.md` for what the Spike validated.
