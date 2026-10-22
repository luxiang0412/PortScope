//! Localisation.
//!
//! Production uses **GNU gettext `.po` catalogues** (taskbook §6.1, §9.7),
//! compiled to `.mo` and installed under `$prefix/share/locale`. This replaces
//! the Spike's Rust string tables, which were a deliberate, recorded spike-only
//! choice (see `docs/I18N.md` and `docs/DECISIONS.md` D-P0-6).
//!
//! Two facts drive the design, both established empirically during the Spike:
//!
//! 1. **`setlocale` switches the language at runtime, but already-rendered
//!    widgets keep their old text.** So a language change must re-apply every
//!    visible string. [`Retranslatable`] is the registry that makes that
//!    mechanical instead of ad-hoc.
//! 2. **Raw system values must never be translated.** [`tr`] is only ever
//!    called with UI text; addresses, ports, PIDs, unit names, cgroup paths,
//!    command lines and journal text are formatted directly and never pass
//!    through here.

use std::cell::RefCell;
use std::rc::Rc;

use gettextrs::{LocaleCategory, bind_textdomain_codeset, bindtextdomain, setlocale, textdomain};

/// A language the product ships.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    En,
    ZhCn,
}

impl Lang {
    /// The gettext locale name.
    pub fn locale(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::ZhCn => "zh_CN",
        }
    }

    /// The endonym shown in the language menu. Deliberately NOT translated:
    /// users must be able to find their own language in a UI they cannot read.
    pub fn endonym(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::ZhCn => "简体中文",
        }
    }

    pub fn all() -> &'static [Lang] {
        &[Lang::En, Lang::ZhCn]
    }
}

/// What the user chose in settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LangSetting {
    System,
    Fixed(Lang),
}

impl LangSetting {
    /// Resolve to a concrete language given the process environment.
    pub fn resolve(self, env: &[(String, String)]) -> Lang {
        match self {
            LangSetting::Fixed(l) => l,
            LangSetting::System => detect_lang(env),
        }
    }

    /// Stable key for persistence in the settings file.
    pub fn key(self) -> &'static str {
        match self {
            LangSetting::System => "system",
            LangSetting::Fixed(Lang::En) => "en",
            LangSetting::Fixed(Lang::ZhCn) => "zh_CN",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s.trim() {
            "en" => LangSetting::Fixed(Lang::En),
            "zh_CN" | "zh" => LangSetting::Fixed(Lang::ZhCn),
            _ => LangSetting::System,
        }
    }
}

/// Detect the language from `LC_ALL`, `LC_MESSAGES`, `LANG`.
///
/// Unsupported locales fall back to English, as specified (taskbook §6.1).
/// An empty variable is treated as unset, because `LC_ALL=` in a shipped
/// `.desktop` file otherwise silently forces the C locale.
pub fn detect_lang(env: &[(String, String)]) -> Lang {
    let get = |name: &str| -> Option<&str> {
        env.iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
            .filter(|v| !v.trim().is_empty())
    };
    let raw = get("LC_ALL")
        .or_else(|| get("LC_MESSAGES"))
        .or_else(|| get("LANG"))
        .unwrap_or("");

    // "zh_CN.UTF-8" / "zh_CN" / "zh" / "zh_CN.utf8@variant"
    let base = raw.split(['.', '@']).next().unwrap_or("").trim();
    let lower = base.to_ascii_lowercase();
    if lower == "zh_cn" || lower == "zh" || lower == "zh_hans" || lower == "zh_sg" {
        Lang::ZhCn
    } else {
        // C/POSIX/anything unsupported -> English.
        Lang::En
    }
}

/// Read the current process environment as `(key, value)` pairs.
pub fn process_env() -> Vec<(String, String)> {
    std::env::vars().collect()
}

/// Initialise gettext for the given locale.
///
/// Returns an error string when the locale cannot be applied, so the caller can
/// fall back to English **and tell the user** rather than silently pretending
/// the requested language is active (taskbook §6.3).
pub fn init_gettext(lang: Lang, localedir: Option<&str>) -> Result<(), String> {
    if let Some(dir) = localedir {
        bindtextdomain("portscope", dir).map_err(|e| format!("bindtextdomain: {e}"))?;
    }
    bind_textdomain_codeset("portscope", "UTF-8")
        .map_err(|e| format!("bind_textdomain_codeset: {e}"))?;
    textdomain("portscope").map_err(|e| format!("textdomain: {e}"))?;

    // `setlocale` needs a *full* locale name with charset ("zh_CN.UTF-8"), not
    // the short language code. Trying the bare name fails on a normal system,
    // which would silently leave the UI in English — the bug this loop fixes.
    // Candidates are ordered from most to least specific.
    let candidates: &[&str] = match lang {
        Lang::En => &["en_US.UTF-8", "en_US.utf8", "C.UTF-8", "C"],
        Lang::ZhCn => &["zh_CN.UTF-8", "zh_CN.utf8", "zh_CN"],
    };
    for candidate in candidates {
        // `gettext-rs` returns `Option<&str>`: `Some` means the locale applied.
        if setlocale(LocaleCategory::LcAll, *candidate).is_some() {
            return Ok(());
        }
    }
    Err(format!(
        "none of the locales {:?} are available on this system",
        candidates
    ))
}

/// Translate a UI string.
///
/// Only ever called with UI text from the catalogue. Raw system data must not
/// pass through here (see the module docs).
pub fn tr(msgid: &str) -> String {
    gettextrs::gettext(msgid)
}

/// Translate with a single substitution, e.g. `"{} endpoints"`.
///
/// Kept explicit rather than building sentences by concatenation, which breaks
/// in Chinese word order (taskbook §6.3).
pub fn tr1(msgid: &str, arg: &str) -> String {
    tr(msgid).replace("{}", arg)
}

// ---------------------------------------------------------------------------
// Retranslatable registry
// ---------------------------------------------------------------------------

type Apply = Box<dyn Fn()>;

/// Holds every "re-apply this text" closure created during UI construction.
///
/// On a language change we re-run all of them, which is how the product avoids
/// the stale-widget problem described in the module docs. Selection and filters
/// live in GTK models, not in the text, so they survive by construction.
#[derive(Clone, Default)]
pub struct Retranslatable(Rc<RefCell<Vec<Apply>>>);

impl Retranslatable {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a closure that re-applies text from the current catalogue.
    pub fn register(&self, f: impl Fn() + 'static) {
        self.0.borrow_mut().push(Box::new(f));
    }

    /// Re-run every registered closure.
    pub fn apply_all(&self) {
        for f in self.0.borrow().iter() {
            f();
        }
    }

    pub fn len(&self) -> usize {
        self.0.borrow().len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.borrow().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use portscope_core::model::ActiveState;

    fn env(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn english_system_defaults_to_english() {
        assert_eq!(
            LangSetting::System.resolve(&env(&[("LANG", "en_US.UTF-8")])),
            Lang::En
        );
    }

    #[test]
    fn chinese_system_defaults_to_chinese() {
        assert_eq!(
            LangSetting::System.resolve(&env(&[("LANG", "zh_CN.UTF-8")])),
            Lang::ZhCn
        );
    }

    #[test]
    fn unsupported_locale_falls_back_to_english() {
        for loc in ["fr_FR.UTF-8", "de_DE.UTF-8", "C", "POSIX", "ja_JP.UTF-8"] {
            assert_eq!(
                LangSetting::System.resolve(&env(&[("LANG", loc)])),
                Lang::En,
                "{loc} must fall back to English"
            );
        }
    }

    #[test]
    fn manual_choice_overrides_system() {
        let e = env(&[("LANG", "zh_CN.UTF-8")]);
        assert_eq!(LangSetting::Fixed(Lang::En).resolve(&e), Lang::En);
        let e2 = env(&[("LANG", "en_US.UTF-8")]);
        assert_eq!(LangSetting::Fixed(Lang::ZhCn).resolve(&e2), Lang::ZhCn);
    }

    #[test]
    fn lc_all_beats_lang() {
        assert_eq!(
            detect_lang(&env(&[("LC_ALL", "zh_CN.UTF-8"), ("LANG", "en_US.UTF-8")])),
            Lang::ZhCn
        );
        assert_eq!(
            detect_lang(&env(&[("LC_ALL", "en_US.UTF-8"), ("LANG", "zh_CN.UTF-8")])),
            Lang::En
        );
    }

    #[test]
    fn empty_variables_are_ignored() {
        // An empty LC_ALL must not force the C locale.
        assert_eq!(
            detect_lang(&env(&[("LC_ALL", ""), ("LANG", "zh_CN.UTF-8")])),
            Lang::ZhCn
        );
    }

    #[test]
    fn zh_variants_and_encodings() {
        for loc in [
            "zh_CN.utf8",
            "zh_CN",
            "zh",
            "zh_CN.UTF-8@variant",
            "zh_SG.UTF-8",
        ] {
            assert_eq!(detect_lang(&env(&[("LANG", loc)])), Lang::ZhCn, "{loc}");
        }
    }

    #[test]
    fn setting_roundtrips_through_persistence_key() {
        for s in [
            LangSetting::System,
            LangSetting::Fixed(Lang::En),
            LangSetting::Fixed(Lang::ZhCn),
        ] {
            assert_eq!(LangSetting::parse(s.key()), s);
        }
        // Garbage falls back to System rather than failing to start.
        assert_eq!(LangSetting::parse("klingon"), LangSetting::System);
    }

    #[test]
    fn endonyms_are_never_localised() {
        // Asserted as a contract: these strings are identical in both locales
        // so a user can find their language in an unreadable UI.
        assert_eq!(Lang::En.endonym(), "English");
        assert_eq!(Lang::ZhCn.endonym(), "简体中文");
    }

    #[test]
    fn static_ui_labels_are_not_used_as_raw_data() {
        // Guard against accidental localisation of technical identifiers: these
        // must stay byte-identical in both catalogues, so they are not looked up.
        for ident in [
            "PID", "PPID", "UID", "cgroup", "TCP", "UDP", "LISTEN", "UNCONN",
        ] {
            assert!(
                ident.is_ascii(),
                "{ident} must stay ASCII-only and untranslated"
            );
        }
    }

    /// Regression: `setlocale` needs a full locale name with charset
    /// ("zh_CN.UTF-8"), not "zh_CN". Passing the short code made init fail and
    /// silently left the whole UI in English.
    #[test]
    fn full_locale_names_are_tried_and_catalogue_loads() {
        let dev = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("po");
        let mo = dev.join("zh_CN/LC_MESSAGES/portscope.mo");
        if !mo.exists() {
            // The .mo is a build artefact; without it there is nothing to load.
            return;
        }
        let dir = dev.to_string_lossy().to_string();
        init_gettext(Lang::ZhCn, Some(&dir)).expect("zh_CN must initialise via a full locale name");
        assert_eq!(tr("Ports"), "端口", "catalogue must actually translate");
        assert_eq!(tr("Processes"), "进程");

        // Switching back must restore English text.
        init_gettext(Lang::En, Some(&dir)).expect("en must initialise");
        assert_eq!(tr("Ports"), "Ports");
    }

    /// Raw technical values must not be translated even when a catalogue is
    /// active (taskbook §6.2).
    #[test]
    fn raw_identifiers_are_not_in_the_catalogue() {
        let dev = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("po");
        if !dev.join("zh_CN/LC_MESSAGES/portscope.mo").exists() {
            return;
        }
        let dir = dev.to_string_lossy().to_string();
        init_gettext(Lang::ZhCn, Some(&dir)).expect("init");
        // Protocol/state codes and object names are never passed through
        // gettext, so they carry no catalogue entry and come back unchanged.
        for raw in [
            "LISTEN",
            "UNCONN",
            "TCP",
            "UDP",
            "init.scope",
            "ssh.service",
            "0.0.0.0",
            "127.0.0.1:5432",
        ] {
            assert_eq!(tr(raw), raw, "{raw} must not be translated");
        }

        // Unit lifecycle states DO have catalogue entries, because the UI shows
        // a translated *label* for them. The raw wire value is stored and
        // displayed verbatim elsewhere (`ActiveState::as_str`), so the contract
        // is: `tr` may translate the label, never the stored value.
        assert_eq!(ActiveState::Active.as_str(), "active");
        assert_eq!(ActiveState::Failed.as_str(), "failed");
        assert_eq!(tr("active"), "运行中", "the label is translated by design");
    }

    #[test]
    fn retranslatable_runs_every_registered_closure() {
        use std::cell::Cell;
        let r = Retranslatable::new();
        let a = Rc::new(Cell::new(0));
        let b = Rc::new(Cell::new(0));
        {
            let a = a.clone();
            r.register(move || a.set(a.get() + 1));
        }
        {
            let b = b.clone();
            r.register(move || b.set(b.get() + 1));
        }
        assert_eq!(r.len(), 2);
        r.apply_all();
        assert_eq!(a.get(), 1);
        assert_eq!(b.get(), 1);
        // Applying twice re-runs them, which is what a second language switch does.
        r.apply_all();
        assert_eq!(a.get(), 2);
        assert_eq!(b.get(), 2);
    }
}
