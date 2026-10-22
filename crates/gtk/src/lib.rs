//! PortScope desktop application — GTK 4 + libadwaita presentation layer.
//!
//! Layout of this crate:
//!
//! * [`state`] — GTK-free presentation model: rows, filters, freshness,
//!   staleness and error classification. Unit-tested headlessly.
//! * [`i18n`] — gettext catalogue handling, locale detection, and the
//!   retranslatable registry that makes runtime switching work.
//! * [`worker`] — background collection so the main loop never blocks.
//! * [`ui`] — the widgets themselves.
//!
//! The split exists because the Spike put everything in one 1 376-line `main.rs`
//! bound to fictional data. Keeping state and i18n GTK-free means the tricky
//! parts (stale-vs-empty, selection preservation, locale fallback) are tested
//! without a display server.

#![forbid(unsafe_code)]

pub mod i18n;
pub mod logs;
pub mod state;
pub mod ui;
pub mod worker;

pub const APP_ID: &str = "io.github.portscope.PortScope";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
