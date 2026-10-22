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

// GTK requires a reverse-DNS application id whose elements use only lowercase
// ASCII letters, digits, `-` and `_`. The earlier "io.github.portscope.PortScope"
// contained an uppercase element and was silently normalised to
// "portscope.portscope" at runtime, which breaks desktop-file matching,
// GResource paths and single-instance behaviour. Verified with
// `wmctrl -lx` before and after.
pub const APP_ID: &str = "io.github.portscope.app";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    /// GTK silently normalises an invalid application id, which breaks
    /// desktop-file matching and single-instance behaviour without any error.
    /// The earlier `io.github.portscope.PortScope` became `portscope.portscope`
    /// at runtime. This test pins the rules so it cannot regress.
    #[test]
    fn app_id_is_a_valid_gtk_application_id() {
        // Reverse-DNS: at least two dot-separated elements.
        assert!(
            APP_ID.split('.').count() >= 2,
            "app id must be reverse-DNS, got {APP_ID}"
        );
        // Only lowercase ASCII letters, digits, '-' and '_' are allowed.
        assert!(
            APP_ID.chars().all(|c| c.is_ascii_lowercase()
                || c.is_ascii_digit()
                || c == '.'
                || c == '-'
                || c == '_'),
            "app id {APP_ID} must be lowercase and contain only [a-z0-9._-]"
        );
        // Each element must start with a letter.
        for element in APP_ID.split('.') {
            assert!(!element.is_empty(), "empty element in {APP_ID}");
            assert!(
                element.chars().next().unwrap().is_ascii_lowercase(),
                "element {element:?} of {APP_ID} must start with a letter"
            );
        }
    }
}
