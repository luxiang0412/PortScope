# UI_UX_SPEC — PortScope

> Phase 0 draft: the specification Phase 2 must implement. **No production UI
> exists yet**; the Spike prototype (fictional data) is the only implementation,
> and it fails two items below (see §7).

## 1. Character

Native, restrained, professional, information-dense. GTK 4 + libadwaita standard
widgets and system theming. No macOS/Windows imitation, no decorative gradients,
no glassmorphism, no gratuitous animation. Monospace for technical fields, system
UI font for prose. Colour is never the sole carrier of meaning — status is always
also text/label.

## 2. Navigation

Top-level: **Ports**, **Processes**, **Services**. Logs are *not* a top-level
destination: they belong to service details and the diagnostic flow.

## 3. Ports view

Default columns: Protocol, Local address, Port, State, Process, PID, User,
Systemd unit.

Required behaviour:

- Filter by protocol, IP family and state.
- Search across port, address, process name, PID, user and *confirmed* service.
- Sort; select; copy; open details; jump to associated process/service.
- Manual refresh plus auto-refresh at 5 s (default), 10 s, 30 s, or off.
- A refresh preserves the selected object and all filters.
- A failed refresh keeps the last good data and marks it **stale** with the
  time of that successful collection.
- Search and double-click must never trigger side-effecting operations.

## 4. Processes view

Columns: name, PID, user, associated endpoint count, confirmed service link,
information state. Details: command line, PPID, process start time, endpoints.

Command lines may contain secrets: they are **excluded from the diagnostic
summary by default**; copying the full command line is a deliberate, clearly
labelled action. No kill/terminate control in v1.0.

## 5. Services view

Columns: unit name, description, state, MainPID, associated ports, latest result.

State labels must distinguish `active`, `inactive`, `failed`, `activating`,
`deactivating`, `reloading`, `not-found`, `unknown`. **Inactive is not a fault**
and must not be styled as one. Details: state summary, association evidence,
recent logs. No start/stop/restart/edit in v1.0.

## 6. Logs

- Default window: last 15 minutes.
- Priority filter and text search.
- Long entries expandable.
- Copy a single entry or a bounded diagnostic summary.
- Always show: queried time range, last update, and truncation / permission /
  failure state.
- Never load unbounded logs. Raw text is shown verbatim; any PortScope
  interpretation is visually separated from the original.

Distinct displays required for: **entries**, **empty result**, **permission
denied**, and **query failure** — these must not be merged into one empty state.

## 7. Settings

Language (system / English / 简体中文); auto-refresh on-off and 5/10/30 s; the
privacy scope of the diagnostic summary; About with version and help. No theme
colour / layout / dashboard customisation.

## 8. Window, layout, and the known defects

Design intent: wide window = list + details side by side; narrow window =
details as a separate navigation page, or secondary columns hidden — **never** a
main table squeezed to an unusable width. No fixed pixel column widths; consider
font, language, scaling and long values.

Acceptance sizes: **1024×600, 1366×768, 1920×1080 CSS**, at 100 / 125 / 150 / 175
/ 200 % where the environment supports it. Native scaling and text scaling are
**not** the same test and are recorded separately.

### Defects measured at Phase 0 (must be fixed in Phase 2)

| ID | Defect | Evidence |
| --- | --- | --- |
| P0-D2 | At 1024×600 CSS, `GDK_SCALE=1`: only Protocol / Local address / Port visible. **State, Process, PID, Systemd unit unreachable; no horizontal scrollbar.** | a Phase 0 capture (removed before publishing: contained LAN addresses) |
| P0-D3 | "1024×600 unreachable" was a 200 %-scale artifact — at `GDK_SCALE=1` the window reaches exactly 1024×600. | `wmctrl` geometry |
| P0-D4 | GUI renders `SampleData` only; no real collection path. | `main.rs` |

If a window manager cannot honour a requested size, the **requested and actual**
sizes are both recorded; a clamped result is never reported as a pass.

## 9. Keyboard and accessibility

| Key | Action |
| --- | --- |
| `Ctrl+F` | focus the current view's search |
| `Ctrl+R` | refresh the current view |
| `Esc` | close popover / leave search |
| `Enter` | open details of the selection |
| `Up` / `Down` | move list selection |
| `Tab` / `Shift+Tab` | traverse interactive controls |

Requirements: the core workflow must be completable **keyboard-only**; focus is
visible and ordered; controls expose accessible name/role/state; errors are
never colour-only; system font scaling and the reduce-motion preference are
respected; empty-search, permission, stale and failure states all give clear
feedback.

## 10. State vocabulary (shared by all views)

`loading` · `ok` · `empty` · `no-match` · `denied` · `stale` · `failed` ·
`gone` (object vanished mid-session) · `degraded` (partial data).

Each state has a defined text, an icon or non-colour cue, and a recovery action
where one exists (retry, widen filters, explain permissions).
