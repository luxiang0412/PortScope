# Security Policy

## Reporting a vulnerability

Please **do not** open a public issue for a security problem. Report it
privately to the maintainer and allow time for a fix before disclosure.

> A dedicated security contact address will be added here once the project has a
> public repository. Until then, contact the maintainer directly.

Include: affected version, what you observed, and the smallest reproduction you
can manage. Please do not include your real logs, hostnames or IP addresses.

## Supported versions

| Version | Supported |
| --- | --- |
| 0.1.x (development) | best effort |

## Security model

PortScope is designed to be safe to run as an ordinary desktop user:

- **No privileges requested.** No root, no `sudo`, no `pkexec`, no polkit, no
  setuid helper, no daemon.
- **No group modification.** PortScope never adds you to `adm`,
  `systemd-journal` or any other group, and never suggests doing so as a
  requirement.
- **No network access.** No telemetry, no upload, no update check.
- **No persistence of collected data.** Snapshots live in memory only. The only
  file written is a small preferences file (language).
- **Explicit subprocess handling.** Every external command uses an explicit
  argv, a timeout, a cancellation path and an output cap. No shell is ever
  invoked.
- **`unsafe` is forbidden** at crate level (`#![forbid(unsafe_code)]`).

## Out of scope

These are deliberate non-goals, not vulnerabilities:

- PortScope shows only what your own permissions allow. Being unable to read
  another user's process or the system journal is the intended behaviour.
- PortScope does not scan networks or probe ports; it reads local kernel state.
- Anyone who can run code as your user can read everything PortScope reads. That
  is the platform's model, not a PortScope weakness.

## Data handling

The "Copy diagnostic summary" action deliberately **excludes full command
lines**, because they frequently contain tokens, passwords and private paths.
Copying a single command line is a separate, explicit action.
