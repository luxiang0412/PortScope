# SECURITY — PortScope

> Phase 0 draft. Describes the security and privacy model the implementation must
> satisfy. Nothing here is a claim that a control is implemented; see
> `docs/STATUS.md` for actual state.

## 1. Privilege model

- The GUI runs **as the invoking user**. Never root.
- No `sudo`, no `pkexec`, no polkit action, no setuid/setgid helper, no daemon.
- PortScope **never** modifies group membership (`adm`, `systemd-journal`, …).
- Every collected field is allowed to fail independently; a partial permission
  failure must not invalidate other available data.
- Unreadable system journal is reported as a **permission limitation**, never as
  "no logs".
- `hidepid=2` and similar `/proc` restrictions degrade gracefully with an
  explanation.
- A socket with `uid: None` is shown as **owner unknown**. It is *not* inferred
  to be a container endpoint.

## 2. Subprocess discipline

- Explicit argv only. **No `sh -c`**, no shell string building, no interpolation
  of untrusted input into command lines.
- `ss`, `systemctl`, `busctl`, `journalctl` are invoked with fixed argument
  vectors, a **timeout**, a **cancellation path**, and an **output size cap**.
- Results are classified into stable internal error types; raw stderr is
  diagnostic context only and never the product's error code.
- Partial reads and non-UTF-8 output are handled lossily without panicking.

> Status: argv discipline and the no-shell rule are implemented in the Spike.
> **Timeouts, cancellation and output caps are not** (audit P0-D1).

## 3. Data lifecycle and privacy

- No upload, no telemetry, no network access of any kind in v1.0.
- No persistence of process/log snapshots. Diagnostic state lives in memory only.
- No on-disk cache of endpoints, command lines or log text.
- Error logs must not record full command lines or sensitive arguments by
  default.
- Every piece of evidence carries its collection time and source, so a diagnosis
  is auditable and reproducible.

## 4. Diagnostic summary export

- The default summary **excludes full command lines** and unrelated sensitive
  content.
- Copying a full command line is a separate, explicitly labelled user action.
- Before export, the field set is shown or precisely described, so the user knows
  what is leaving the app.
- Journal text included in a summary is clearly delimited from PortScope's own
  interpretation.
- The summary includes collection time and any `degraded` flags, so a recipient
  knows what was missing.

## 5. Non-goals / explicitly refused

Killing processes, starting/stopping/editing units, editing firewall or network
configuration, mounting or reading other users' private files, privilege
escalation of any kind, and auto-remediating permission problems.

## 6. Repository hygiene

Before any public release: no real user logs, machine identifiers, IP lists,
private paths or un-redacted test output in the repository; screenshots reviewed
for personal data; Spike screenshots are fictional-data captures and are safe by
construction, but production screenshots must be reviewed individually.

## 7. Reporting a vulnerability

To be finalised in Phase 6 (`SECURITY.md` at repository root). Until then, do not
publish vulnerability details publicly before the maintainer responds.
