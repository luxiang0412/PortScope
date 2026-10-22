//! PortScope core — GTK-free data collection and model.
//!
//! Deliberately free of any UI dependency so that every association rule and
//! parser can be tested in CI without a display server.
//!
//! Modules
//!   [`model`]     pure data types, no I/O
//!   [`exec`]      bounded subprocess execution (timeout, cancel, output cap)
//!   [`endpoints`] `ss` + `/proc/net` collection and parsing
//!   [`process`]   `/proc` inspection, PID-reuse defence, inode ownership
//!   [`services`]  systemd via D-Bus primary and `systemctl` fallback
//!   [`journal`]   journald via `journalctl -o json` (stderr-aware)
//!   [`assoc`]     association rules with explicit evidence
//!   [`collect`]   snapshot orchestration

#![forbid(unsafe_code)]

pub mod assoc;
pub mod collect;
pub mod endpoints;
pub mod exec;
pub mod journal;
pub mod model;
pub mod process;
pub mod services;

pub use collect::{CollectOptions, collect, collect_cancellable};
pub use exec::{Cancel, ExecLimits};
pub use model::*;
