//! The details pane.
//!
//! Shows the full record for the selected object, including the **evidence**
//! behind every association. Values are raw system data and are never
//! translated; only labels are.

use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use crate::i18n::tr;
use crate::state::{Freshness, InfoState, PortRow, ProcessRow, ServiceRow};
use crate::ui::App;

/// Build the details container. It is a scrolling, rebuildable box.
///
/// The inner content box is stored on `App` so `refresh_selected` can clear and
/// rebuild it without any global registry.
pub fn build_details_box(state: &Rc<App>) -> gtk::Box {
    let outer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    outer.set_width_request(320);

    let scroller = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .hexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .build();

    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    content.set_margin_top(12);
    content.set_margin_bottom(12);
    content.set_margin_start(12);
    content.set_margin_end(12);
    scroller.set_child(Some(&content));

    outer.append(&scroller);
    state.details_content.borrow_mut().replace(content);
    outer
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Field {
    label: String,
    value: String,
    mono: bool,
    /// When set, the value is a state rather than data and is shown dimmed.
    state: Option<InfoState>,
}

fn field(label: impl Into<String>, value: impl Into<String>) -> Field {
    Field {
        label: label.into(),
        value: value.into(),
        mono: false,
        state: None,
    }
}

fn mono_field(label: impl Into<String>, value: impl Into<String>) -> Field {
    Field {
        label: label.into(),
        value: value.into(),
        mono: true,
        state: None,
    }
}

fn state_field(label: impl Into<String>, state: InfoState) -> Field {
    let text = match state {
        InfoState::Known => tr("Loaded"),
        InfoState::Denied => tr("Permission denied"),
        InfoState::NotQueried => tr("Not queried"),
        InfoState::Gone => tr("No longer running"),
        InfoState::Unknown => tr("Unknown"),
    };
    Field {
        label: label.into(),
        value: text,
        mono: false,
        state: Some(state),
    }
}

/// Rebuild the details pane for whatever is currently selected.
pub fn refresh_selected(state: &Rc<App>) {
    let Some(content) = state.details_content.borrow().clone() else {
        return;
    };

    while let Some(child) = content.first_child() {
        content.remove(&child);
    }

    // Copy the view out: holding the `view` borrow across the render calls,
    // which themselves borrow other `App` fields, risks a double borrow.
    let view = *state.view.borrow();
    match view {
        crate::state::View::Ports => {
            let sel = state.selected_port.borrow().clone();
            render_port(&content, state, sel.as_ref());
        }
        crate::state::View::Processes => {
            let sel = state.selected_process.borrow().clone();
            render_process(&content, state, sel.as_ref());
        }
        crate::state::View::Services => {
            let sel = state.selected_service.borrow().clone();
            render_service(&content, state, sel.as_ref());
        }
    }
}

fn heading(text: &str) -> gtk::Label {
    let l = gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .max_width_chars(30)
        .build();
    l.add_css_class("title-4");
    l
}

fn section(title: &str) -> gtk::Label {
    let l = gtk::Label::builder()
        .label(title)
        .xalign(0.0)
        .margin_top(6)
        .build();
    l.add_css_class("heading");
    l
}

fn render_fields(content: &gtk::Box, fields: &[Field]) {
    let grid = gtk::Grid::builder()
        .column_spacing(12)
        .row_spacing(6)
        .hexpand(true)
        .build();
    for (i, f) in fields.iter().enumerate() {
        let label = gtk::Label::builder()
            .label(&f.label)
            .xalign(0.0)
            .valign(gtk::Align::Start)
            .build();
        label.add_css_class("field-label");

        // `wrap_mode(WordChar)` breaks long cgroup paths and IPv6 addresses
        // that have no spaces, instead of pushing the pane wider.
        let value = gtk::Label::builder()
            .label(&f.value)
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .selectable(true)
            .hexpand(true)
            .build();
        if f.mono {
            value.add_css_class("mono");
        }
        if let Some(state) = f.state
            && !state.is_known()
        {
            // A non-known state must not look like data.
            value.add_css_class("dim");
        }

        grid.attach(&label, 0, i as i32, 1, 1);
        grid.attach(&value, 1, i as i32, 1, 1);
    }
    content.append(&grid);
}

fn render_evidence(content: &gtk::Box, evidence: &[String]) {
    content.append(&section(&tr("Evidence")));
    if evidence.is_empty() {
        let l = gtk::Label::builder()
            .label(tr("No confirmable association"))
            .xalign(0.0)
            .wrap(true)
            .build();
        l.add_css_class("dim");
        content.append(&l);
        return;
    }
    for e in evidence {
        let l = gtk::Label::builder()
            .label(e)
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .selectable(true)
            .hexpand(true)
            .build();
        l.add_css_class("evidence");
        content.append(&l);
    }
}

fn render_port(content: &gtk::Box, _state: &Rc<App>, sel: Option<&PortRow>) {
    let Some(p) = sel else {
        let l = gtk::Label::builder()
            .label(tr("Select a row to see details."))
            .xalign(0.0)
            .wrap(true)
            .build();
        l.add_css_class("dim");
        content.append(&heading(&tr("Details")));
        content.append(&l);
        return;
    };

    let title = format!(
        "{} {}:{}",
        p.protocol.as_str().to_uppercase(),
        p.local_addr,
        p.port
    );
    content.append(&heading(&title));

    let mut fields = vec![
        mono_field(tr("Protocol"), p.protocol.as_str().to_uppercase()),
        mono_field(tr("Local address"), p.local_addr.clone()),
        mono_field(tr("Port"), p.port.to_string()),
        mono_field(tr("State"), p.state.label()),
    ];
    match p.uid {
        Some(u) => fields.push(mono_field(tr("UID"), u.to_string())),
        None => fields.push(state_field(tr("UID"), InfoState::Unknown)),
    }
    match &p.user {
        Some(u) => fields.push(field(tr("User"), u.clone())),
        None => fields.push(state_field(tr("User"), InfoState::Unknown)),
    }
    match p.pid {
        Some(pid) => {
            fields.push(mono_field(tr("PID"), pid.to_string()));
        }
        None => {
            // Honest: an absent PID means "we could not see it", not "no owner".
            fields.push(state_field(tr("PID"), InfoState::Denied));
        }
    }
    match &p.process {
        Some(n) => fields.push(field(tr("Process"), n.clone())),
        None => fields.push(state_field(tr("Process"), InfoState::Unknown)),
    }
    render_fields(content, &fields);

    // Raw socket facts, useful for diagnosis and never translated.
    content.append(&section(&tr("cgroup")));

    render_evidence(content, &p.evidence);

    let note = gtk::Label::builder()
        .label(tr(
            "Raw values are shown exactly as reported by the system and are not translated.",
        ))
        .xalign(0.0)
        .wrap(true)
        .margin_top(8)
        .build();
    note.add_css_class("dim");
    content.append(&note);
}

fn render_process(content: &gtk::Box, _state: &Rc<App>, sel: Option<&ProcessRow>) {
    let Some(p) = sel else {
        content.append(&heading(&tr("Details")));
        return;
    };
    content.append(&heading(&format!("{} ({})", p.name, p.pid)));

    let mut fields = vec![
        field(tr("Name"), p.name.clone()),
        mono_field(tr("PID"), p.pid.to_string()),
    ];
    match p.uid {
        Some(u) => fields.push(mono_field(tr("UID"), u.to_string())),
        None => fields.push(state_field(tr("UID"), InfoState::Unknown)),
    }
    match &p.user {
        Some(u) => fields.push(field(tr("User"), u.clone())),
        None => fields.push(state_field(tr("User"), InfoState::Unknown)),
    }
    match p.ppid {
        Some(v) => fields.push(mono_field(tr("PPID"), v.to_string())),
        None => fields.push(state_field(tr("PPID"), InfoState::Unknown)),
    }
    match p.start_time_ticks {
        Some(v) => fields.push(mono_field(tr("Start time"), v.to_string())),
        None => fields.push(state_field(tr("Start time"), InfoState::Unknown)),
    }
    fields.push(mono_field(tr("Endpoints"), p.endpoint_count.to_string()));

    // The command line may contain secrets, so its absence must be explained
    // rather than shown as an empty string (§5.4).
    match &p.cmdline {
        Some(argv) => fields.push(mono_field(tr("Command line"), argv.join(" "))),
        None => fields.push(state_field(tr("Command line"), p.cmdline_state)),
    }

    render_fields(content, &fields);

    if !p.units.is_empty() {
        content.append(&section(&tr("Unit")));
        for u in &p.units {
            let l = gtk::Label::builder()
                .label(u)
                .xalign(0.0)
                .selectable(true)
                .build();
            l.add_css_class("mono");
            content.append(&l);
        }
    }
}

fn render_service(content: &gtk::Box, state: &Rc<App>, sel: Option<&ServiceRow>) {
    let Some(s) = sel else {
        content.append(&heading(&tr("Details")));
        return;
    };
    content.append(&heading(&s.unit));

    let mut fields = vec![
        mono_field(tr("Unit"), s.unit.clone()),
        field(
            tr("Scope"),
            match s.scope {
                portscope_core::model::UnitScope::System => tr("system"),
                portscope_core::model::UnitScope::User => tr("user"),
            },
        ),
        // The state *value* stays the raw systemd string; the label is translated.
        field(tr("State"), tr(s.active_state.as_str())),
        mono_field(tr("Sub state"), s.sub_state.clone()),
        mono_field(tr("Loaded"), s.load_state.clone()),
    ];
    match s.main_pid {
        Some(pid) => fields.push(mono_field(tr("Main PID"), pid.to_string())),
        None => fields.push(state_field(tr("Main PID"), s.detail_state)),
    }
    render_fields(content, &fields);

    if !s.ports.is_empty() {
        content.append(&section(&tr("Associated ports")));
        for (proto, addr, port) in &s.ports {
            let l = gtk::Label::builder()
                .label(format!("{} {}:{}", proto.as_str(), addr, port))
                .xalign(0.0)
                .selectable(true)
                .build();
            l.add_css_class("mono");
            content.append(&l);
        }
    }

    if !s.triggers.is_empty() {
        content.append(&section(&tr("Unit")));
        for t in &s.triggers {
            let l = gtk::Label::builder()
                .label(t)
                .xalign(0.0)
                .selectable(true)
                .build();
            l.add_css_class("mono");
            content.append(&l);
        }
    }

    render_evidence(content, &s.evidence);

    // Logs are fetched asynchronously; this only creates the placeholder.
    content.append(&section(&tr("Recent logs")));
    let logs_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let status = gtk::Label::builder()
        .label(tr("Refreshing…"))
        .xalign(0.0)
        .wrap(true)
        .build();
    status.add_css_class("dim");
    logs_box.append(&status);
    content.append(&logs_box);

    // Cache the logs per unit.
    //
    // Without this, every auto-refresh (5 s) rebuilt the pane, cancelled the
    // in-flight `journalctl`, and started again — so a query that takes longer
    // than the refresh interval could never finish. That was a real bug: the
    // logs section stayed permanently on its placeholder.
    let key = (s.scope, s.unit.clone());
    let cached = state
        .log_cache
        .borrow()
        .get(&key)
        .map(|c| c.outcome.clone());

    if let Some(outcome) = cached {
        render_logs(&logs_box, &outcome);
        return;
    }

    // Query the last 15 minutes (taskbook §5.6) for the selected unit.
    let (generation, rx) = state.logs.fetch(s.scope, s.unit.clone(), 15 * 60, 200);
    *state.log_generation.borrow_mut() = generation;

    let logs_box_for_poll = logs_box.clone();
    let state_for_poll = state.clone();
    glib::timeout_add_local(std::time::Duration::from_millis(120), move || {
        // Abandon the poll if a newer query has superseded this one.
        if *state_for_poll.log_generation.borrow() != generation {
            return glib::ControlFlow::Break;
        }
        match rx.try_recv() {
            Ok(res) => {
                render_logs(&logs_box_for_poll, &res.outcome);
                state_for_poll.log_cache.borrow_mut().insert(
                    key.clone(),
                    LogEntryCache {
                        outcome: res.outcome,
                    },
                );
                glib::ControlFlow::Break
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
        }
    });
}

/// Cached log outcome for one unit.
#[derive(Clone)]
pub struct LogEntryCache {
    pub outcome: crate::logs::LogOutcome,
}

/// Render a journal outcome.
///
/// The four outcomes are deliberately rendered *differently*: a permission
/// denial is not an empty log, and saying "no entries" when we were refused
/// read access would be exactly the kind of plausible lie the taskbook forbids
/// (§4.6, §9.3).
fn render_logs(container: &gtk::Box, outcome: &crate::logs::LogOutcome) {
    use crate::logs::LogOutcome;

    while let Some(child) = container.first_child() {
        container.remove(&child);
    }

    match outcome {
        LogOutcome::Entries(entries) => {
            let header = gtk::Label::builder()
                .label(tr("Last 15 minutes"))
                .xalign(0.0)
                .build();
            header.add_css_class("dim");
            container.append(&header);

            for e in entries.iter().take(200) {
                let row = gtk::Box::new(gtk::Orientation::Vertical, 2);
                let meta = gtk::Label::builder()
                    .label(format!(
                        "{} {}",
                        format_ts(e.timestamp_usec),
                        e.priority_label()
                    ))
                    .xalign(0.0)
                    .build();
                meta.add_css_class("dim");
                meta.add_css_class("mono");

                // Journal text is shown verbatim and never translated.
                let msg = gtk::Label::builder()
                    .label(&e.message)
                    .xalign(0.0)
                    .wrap(true)
                    .wrap_mode(gtk::pango::WrapMode::WordChar)
                    .selectable(true)
                    .build();
                msg.add_css_class("mono");

                row.append(&meta);
                row.append(&msg);
                container.append(&row);
            }

            let note = gtk::Label::builder()
                .label(tr("Log text is shown verbatim and is never translated."))
                .xalign(0.0)
                .wrap(true)
                .build();
            note.add_css_class("dim");
            container.append(&note);
        }
        LogOutcome::Empty => {
            let l = gtk::Label::builder()
                .label(tr("No log entries in this window."))
                .xalign(0.0)
                .wrap(true)
                .build();
            l.add_css_class("dim");
            container.append(&l);
        }
        LogOutcome::Denied(hint) => {
            // Permission: keep the explanation and the raw reason.
            let l = gtk::Label::builder()
                .label(tr(
                    "The system journal is not readable by this user, so logs are unavailable. Other diagnostics still work.",
                ))
                .xalign(0.0)
                .wrap(true)
                .build();
            container.append(&l);
            let raw = gtk::Label::builder()
                .label(hint)
                .xalign(0.0)
                .wrap(true)
                .selectable(true)
                .build();
            raw.add_css_class("dim");
            raw.add_css_class("evidence");
            container.append(&raw);
        }
        LogOutcome::Failed(e) => {
            let l = gtk::Label::builder()
                .label(tr("Collection failed"))
                .xalign(0.0)
                .wrap(true)
                .build();
            container.append(&l);
            let raw = gtk::Label::builder()
                .label(e)
                .xalign(0.0)
                .wrap(true)
                .selectable(true)
                .build();
            raw.add_css_class("dim");
            raw.add_css_class("evidence");
            container.append(&raw);
        }
        LogOutcome::Cancelled => {
            let l = gtk::Label::builder()
                .label(tr("Refresh cancelled"))
                .xalign(0.0)
                .wrap(true)
                .build();
            l.add_css_class("dim");
            container.append(&l);
        }
    }
}

/// Format a journal timestamp (`__REALTIME_TIMESTAMP`, microseconds) for display.
///
/// Deliberately simple and locale-independent (UTC `HH:MM:SS`): reformatting into
/// a locale date would be a translation of raw data, which is out of scope here.
fn format_ts(usec: u64) -> String {
    let secs = (usec / 1_000_000) as i64;
    let time_of_day = secs.rem_euclid(86_400);
    format!(
        "{:02}:{:02}:{:02}",
        time_of_day / 3600,
        (time_of_day % 3600) / 60,
        time_of_day % 60
    )
}

// ---------------------------------------------------------------------------
// Diagnostic summary
// ---------------------------------------------------------------------------

/// Copy a bounded, privacy-conscious diagnostic summary.
///
/// Deliberately **excludes full command lines**: they routinely contain
/// passwords, tokens and paths (taskbook §5.4, §7.3).
pub fn copy_diagnostic_summary(state: &Rc<App>) {
    let summary = super::summary::diagnostic_summary(&state.state.borrow());
    if let Some(display) = gtk::gdk::Display::default() {
        display.clipboard().set_text(&summary);
    }
    if let Some(toast) = state.toast.borrow().as_ref() {
        toast.add_toast(adw::Toast::new(&tr(
            "Diagnostic summary copied to the clipboard.",
        )));
    }
}

/// Timestamp helper used by the summary and the status bar.
pub fn format_freshness(f: Freshness) -> &'static str {
    match f {
        Freshness::Fresh => "fresh",
        Freshness::Stale => "stale",
        Freshness::Unknown => "unknown",
    }
}

#[cfg(test)]
mod tests {
    // Details rendering needs a display for widgets, so the *decisions* it makes
    // are tested in `state` instead. This test only guards the summary helper's
    // privacy contract, which is pure logic.
    use crate::state::AppState;
    use crate::ui::summary::diagnostic_summary;
    use portscope_core::model::*;

    #[test]
    fn summary_excludes_full_command_lines() {
        let mut snap = Snapshot::default();
        snap.processes.push(ProcessInfo {
            pid: 7,
            name: "secretive".into(),
            cmdline: Some(vec!["secretive".into(), "--password=hunter2".into()]),
            uid: Some(1000),
            user: Some("me".into()),
            ppid: Some(1),
            start_time_ticks: Some(1),
            source: PidSource::TestHarness,
        });
        let mut st = AppState::default();
        st.apply_snapshot(snap);
        let s = diagnostic_summary(&st);
        assert!(
            !s.contains("hunter2"),
            "summary must not leak command-line secrets"
        );
        assert!(s.contains("secretive"), "process name is expected");
    }
}
