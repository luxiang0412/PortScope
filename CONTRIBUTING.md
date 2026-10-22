# Contributing to PortScope

## Development environment

| Tool | Version |
| --- | --- |
| Rust | ≥ 1.85 (1.94.1 is pinned in `rust-toolchain.toml`) |
| GTK | ≥ 4.14 |
| libadwaita | ≥ 1.5 |
| gettext | any (`msgfmt`) |

```bash
sudo apt install libgtk-4-dev libadwaita-1-dev gettext build-essential
```

> Ubuntu 24.04's `rustc` package is 1.75 and cannot build this project. Use
> [rustup](https://rustup.rs/).

## Before you open a pull request

All four must pass locally; CI enforces the same:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -- --test-threads=1
msgfmt --check-format -o /tmp/check.mo crates/gtk/po/zh_CN.po
```

### Test rules

1. **Tests must not depend on a service that happens to be running.** Create
   ephemeral listeners; use throw-away `--user` systemd units.
2. **A skipped test must log its reason.** A silent skip is treated as a failure.
3. **Fictional data is for deterministic UI tests only.** Never let a test that
   uses fictional data be presented as proof that real collection works.
4. A non-display test belongs in `crates/core` or `crates/gtk/src/state.rs`, not
   in widget code, so CI can run it headlessly.

## The one rule that must not be broken

**An endpoint is never associated with a service by guessing.**

No port-number matching, no process-name ↔ unit-name matching, no description
text matching. Every link must carry the [`Evidence`] that produced it. If you
add an association path, add a test proving that the *wrong* association does
not happen (see `no_cgroup_means_no_unit_association_never_port_heuristic`).

## Untranslated data

IP addresses, ports, PIDs, unit names, cgroup paths, command lines and journal
text must be shown **verbatim** in both languages. Only UI text goes through
`tr()`. If you add a string, add it to every `crates/gtk/po/*.po` with a
translator comment where the term is ambiguous.

## Error handling

A field that cannot be read is not an empty value. Use
`CollectionError` with a stable code and let the UI distinguish *denied*,
*unknown*, *not queried*, *gone* and *failed*. Do not collapse them.

## Subprocesses

Never call `Command::output()` directly. Use `core::exec::run_bounded` or
`run_checked`, which enforce a timeout, cancellation and output caps. Never use
a shell.

## Scope

Out of scope for v1.0: killing processes, starting/stopping/editing units,
firewall changes, running as root, automatic group modification, network
scanning, remote hosts, telemetry.

## Commit messages

Explain *why*, and state what you verified. If something is unverified, say so
in the message rather than implying it works.
