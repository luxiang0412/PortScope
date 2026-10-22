//! Settings window (taskbook §5.7).
//!
//! Scope is deliberately small: language, auto-refresh, the privacy scope of the
//! diagnostic summary, and About. The taskbook explicitly forbids adding theme
//! colour / layout / dashboard options without demonstrated user value.
//!
//! Every control writes through the same `App` fields the rest of the UI reads,
//! so there is one source of truth per setting.

use std::rc::Rc;

use adw::prelude::*;

use crate::i18n::{Lang, LangSetting, tr};
use crate::ui::{App, apply_language_setting, refresh_now};

/// Build and present the settings window.
pub fn present(state: &Rc<App>) {
    let Some(layout) = state.layout.borrow().as_ref().cloned() else {
        return;
    };

    let window = adw::PreferencesWindow::builder()
        .transient_for(&layout.window)
        .modal(false)
        .title(tr("Settings"))
        .default_width(640)
        .default_height(560)
        .build();

    // --- language ---------------------------------------------------------
    let lang_page = adw::PreferencesPage::new();
    let lang_group = adw::PreferencesGroup::builder()
        .title(tr("Language"))
        .build();

    let current = *state.lang_setting.borrow();
    let lang_row = adw::ComboRow::builder()
        .title(tr("Language"))
        .subtitle(tr(
            "The manual choice overrides the system language and is kept after restart.",
        ))
        .model(&gtk::StringList::new(&[
            // Short label; the full explanation lives in the subtitle so the
            // value is not truncated to "Syste..." in a narrow window.
            tr("System").as_str(),
            Lang::En.endonym(),
            Lang::ZhCn.endonym(),
        ]))
        .selected(match current {
            LangSetting::System => 0,
            LangSetting::Fixed(Lang::En) => 1,
            LangSetting::Fixed(Lang::ZhCn) => 2,
        })
        .build();
    {
        let state = state.clone();
        lang_row.connect_selected_notify(move |row| {
            let setting = match row.selected() {
                1 => LangSetting::Fixed(Lang::En),
                2 => LangSetting::Fixed(Lang::ZhCn),
                _ => LangSetting::System,
            };
            if *state.lang_setting.borrow() == setting {
                return;
            }
            apply_language_setting(&state, setting);
        });
    }
    // The combo is right-aligned against a two-line subtitle, which squeezed
    // "System default" down to "Syste...". Reserving a minimum keeps the
    // selected value readable in both languages.
    //
    // libadwaita 1.5 exposes no public accessor for the internal dropdown (that
    // arrives later), so the widget is located by walking; if the internal
    // structure changes the request is simply skipped rather than panicking.
    reserve_combo_width(&lang_row, 180);
    lang_group.add(&lang_row);
    lang_page.add(&lang_group);

    // --- refresh ----------------------------------------------------------
    let refresh_group = adw::PreferencesGroup::builder()
        .title(tr("Refresh"))
        .build();

    let interval = *state.refresh_interval.borrow();
    let refresh_row = adw::ComboRow::builder()
        .title(tr("Auto-refresh"))
        .subtitle(tr(
            "A refresh keeps your selection and filters, and marks data stale if it fails.",
        ))
        .model(&gtk::StringList::new(&[
            tr("Off").as_str(),
            tr("5 seconds").as_str(),
            tr("10 seconds").as_str(),
            tr("30 seconds").as_str(),
        ]))
        .selected(match interval {
            None => 0,
            Some(5) => 1,
            Some(10) => 2,
            Some(30) => 3,
            // An unexpected stored value falls back to the documented default.
            Some(_) => 1,
        })
        .build();
    {
        let state = state.clone();
        refresh_row.connect_selected_notify(move |row| {
            let value = match row.selected() {
                0 => None,
                2 => Some(10u32),
                3 => Some(30u32),
                _ => Some(5u32),
            };
            *state.refresh_interval.borrow_mut() = value;
        });
    }

    let now_row = adw::ActionRow::builder()
        .title(tr("Refresh"))
        .subtitle(tr("Collect the current state immediately."))
        .build();
    let now_btn = gtk::Button::builder()
        .label(tr("Refresh"))
        .valign(gtk::Align::Center)
        .build();
    {
        let state = state.clone();
        now_btn.connect_clicked(move |_| refresh_now(&state));
    }
    now_row.add_suffix(&now_btn);
    now_row.set_activatable_widget(Some(&now_btn));

    refresh_group.add(&refresh_row);
    refresh_group.add(&now_row);
    lang_page.add(&refresh_group);

    // --- privacy ----------------------------------------------------------
    let privacy_group = adw::PreferencesGroup::builder()
        .title(tr("Diagnostic summary contents"))
        .build();
    let privacy_row = adw::ActionRow::builder()
        .title(tr("Diagnostic summary contents"))
        .subtitle(tr(
            "The summary includes ports, processes, services, evidence and collection time. It does not include full command lines.",
        ))
        .build();
    privacy_row.set_subtitle_lines(4);
    privacy_group.add(&privacy_row);

    let privacy_note = adw::ActionRow::builder()
        .title(tr(
            "Raw values are shown exactly as reported by the system and are not translated.",
        ))
        .build();
    privacy_note.set_subtitle_lines(3);
    privacy_group.add(&privacy_note);
    lang_page.add(&privacy_group);

    window.add(&lang_page);

    window.present();
}

/// Give an `AdwComboRow`'s dropdown a minimum width.
fn reserve_combo_width(row: &adw::ComboRow, px: i32) {
    fn find(w: &gtk::Widget) -> Option<gtk::DropDown> {
        if let Some(d) = w.downcast_ref::<gtk::DropDown>() {
            return Some(d.clone());
        }
        let mut child = w.first_child();
        while let Some(c) = child {
            if let Some(found) = find(&c) {
                return Some(found);
            }
            child = c.next_sibling();
        }
        None
    }
    if let Some(dropdown) = find(row.upcast_ref::<gtk::Widget>()) {
        dropdown.set_size_request(px, -1);
    }
}
