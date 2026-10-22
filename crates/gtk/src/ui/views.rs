//! The three list views: Ports, Processes, Services.
//!
//! Design notes:
//!
//! * Columns use character-based widths plus `expand` rather than fixed pixels
//!   (the Spike's D10 lesson), and the table scrolls horizontally only as a
//!   safety net — not as the layout strategy.
//! * Filtering is applied by rebuilding the `ListStore` contents, which keeps
//!   the "preserve selection by identity" logic in one place (`refill_all`).
//! * Column titles are stored so a language change can re-title them in place.

use std::rc::Rc;

use adw::prelude::*;

use crate::i18n::tr;
use crate::state::{PortRow, ProcessRow, ServiceRow, matches_query};
use crate::ui::{App, PortObj, ProcObj, SvcObj, show_details};

/// A built view plus the handles needed to re-title and re-filter it.
pub struct BuiltView {
    pub widget: gtk::Widget,
    pub view: gtk::ColumnView,
    /// `(column, msgid)` pairs so titles can be re-applied on language change.
    pub titles: Vec<(gtk::ColumnViewColumn, &'static str)>,
    pub empty_status: adw::StatusPage,
    /// Secondary columns hidden in the narrow layout. Hidden, not deleted, so
    /// the data is still reachable via the details pane (taskbook §5.8).
    pub secondary: Vec<gtk::ColumnViewColumn>,
}

impl BuiltView {
    /// Show or hide the secondary columns.
    ///
    /// At 1024×600 eight columns cannot all stay readable. Rather than shrinking
    /// every column until the values are "…" (the Spike's P0-D2), the narrow
    /// layout drops the secondary ones and the details pane carries the rest.
    pub fn set_secondary_visible(&self, visible: bool) {
        for c in &self.secondary {
            c.set_visible(visible);
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Column factory producing an ellipsised label.
fn text_col<T: Clone + 'static>(
    title: &str,
    extract: impl Fn(&T) -> String + 'static,
    chars: i32,
    expand: bool,
    mono: bool,
) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    let min_chars = chars.max(3);
    factory.connect_setup(move |_, obj| {
        let Some(item) = obj.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        // `width_chars` is the column's real floor. Without it GTK treats the
        // label as shrinkable and renders just "…" — the defect seen when the
        // numeric columns were squeezed.
        let label = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .width_chars(min_chars)
            .build();
        if mono {
            label.add_css_class("mono");
        }
        item.set_child(Some(&label));
    });
    let extract = Rc::new(extract);
    factory.connect_bind(move |_, obj| {
        let Some(item) = obj.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(label) = item.child().and_then(|c| c.downcast::<gtk::Label>().ok()) else {
            return;
        };
        let text = item
            .item()
            .and_then(|o| o.downcast::<gtk::glib::BoxedAnyObject>().ok())
            .and_then(|b| {
                // `BoxedAnyObject::borrow` panics if the stored type differs
                // from T. Each store holds exactly one row type, so this is a
                // programming error rather than a runtime condition; still,
                // avoid taking the whole app down if it ever happens.
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let t = b.borrow::<T>();
                    extract(&t)
                }))
                .ok()
            })
            .unwrap_or_default();
        label.set_text(&text);
    });

    // `fixed_width` alone makes GTK shrink the column below its content when
    // space is tight, which is what turned "Protocol" into "…" at 1024x600.
    // Setting the factory's own minimum keeps the column readable and lets the
    // *expanding* column absorb the leftover space instead.
    let column = gtk::ColumnViewColumn::builder()
        .title(title)
        .factory(&factory)
        .expand(expand)
        .resizable(true)
        .build();
    // Width must cover the header *and* typical values: "Port" is 4 characters
    // but the value can be 5 digits, and a column clipped to the header is
    // exactly the "5…" defect. `chars` is the caller's value budget, so the
    // floor is max(header, chars).
    // CJK glyphs are roughly twice as wide as Latin ones, so a header like
    // "端口" needs more room than its 2 characters suggest. Estimate width from
    // the glyphs actually present rather than from `chars().count()`.
    let header_width: i32 = title
        .chars()
        .map(|c| if c.is_ascii() { 9 } else { 18 })
        .sum();
    let value_width = chars * 9;
    column.set_fixed_width(header_width.max(value_width) + 20);
    column
}

fn scrolled(view: &gtk::ColumnView) -> gtk::ScrolledWindow {
    gtk::ScrolledWindow::builder()
        .child(view)
        .vexpand(true)
        .hexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Automatic)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .build()
}

fn empty_status() -> adw::StatusPage {
    adw::StatusPage::builder()
        .icon_name("system-search-symbolic")
        .title(tr("Nothing to show"))
        .description(tr("No endpoints were reported by the system."))
        .build()
}

/// Stack the table with an empty-state page.
fn wrap(view: &gtk::ColumnView, status: &adw::StatusPage) -> gtk::Widget {
    let stack = gtk::Stack::builder()
        .vexpand(true)
        .hexpand(true)
        .transition_type(gtk::StackTransitionType::Crossfade)
        .build();
    stack.add_named(&scrolled(view), Some("table"));
    stack.add_named(status, Some("empty"));
    stack.set_visible_child_name("table");
    stack.upcast()
}

// ---------------------------------------------------------------------------
// Ports
// ---------------------------------------------------------------------------

pub fn build_ports_view(state: &Rc<App>) -> BuiltView {
    let selection = gtk::SingleSelection::new(Some(state.stores.ports.clone()));
    // No autoselect (we want an explicit empty state at startup), but clicking a
    // row must keep it selected. `can_unselect(true)` made every click on an
    // already-selected row toggle the details pane off.
    selection.set_autoselect(false);
    selection.set_can_unselect(false);
    let view = gtk::ColumnView::builder()
        .model(&selection)
        .show_row_separators(true)
        .show_column_separators(true)
        .build();

    let specs: Vec<(&'static str, gtk::ColumnViewColumn)> = vec![
        (
            "Protocol",
            text_col::<PortRow>(
                &tr("Protocol"),
                |p| p.protocol.as_str().to_ascii_uppercase(),
                5,
                false,
                true,
            ),
        ),
        (
            "Local address",
            text_col::<PortRow>(
                &tr("Local address"),
                |p| p.local_addr.clone(),
                16,
                true,
                true,
            ),
        ),
        (
            "Port",
            text_col::<PortRow>(&tr("Port"), |p| p.port.to_string(), 7, false, true),
        ),
        (
            "State",
            text_col::<PortRow>(&tr("State"), |p| p.state.label(), 9, false, true),
        ),
        (
            "Process",
            text_col::<PortRow>(
                &tr("Process"),
                |p| match (&p.process, p.degraded) {
                    (Some(n), _) => n.clone(),
                    // "Unknown" here is a real information state: the socket may
                    // well have an owner we simply cannot name (§4.4).
                    (None, true) => tr("Unknown"),
                    (None, false) => "—".to_string(),
                },
                14,
                true,
                false,
            ),
        ),
        (
            "PID",
            text_col::<PortRow>(
                &tr("PID"),
                |p| {
                    p.pid
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "—".to_string())
                },
                8,
                false,
                true,
            ),
        ),
        (
            "User",
            text_col::<PortRow>(
                &tr("User"),
                |p| {
                    p.user
                        .clone()
                        .or_else(|| p.uid.map(|u| u.to_string()))
                        .unwrap_or_else(|| tr("Unknown"))
                },
                10,
                false,
                false,
            ),
        ),
        (
            "Systemd unit",
            text_col::<PortRow>(
                &tr("Systemd unit"),
                |p| {
                    p.unit
                        .clone()
                        .unwrap_or_else(|| tr("No confirmable association"))
                },
                22,
                true,
                true,
            ),
        ),
    ];

    let mut titles = Vec::new();
    for (msgid, col) in &specs {
        view.append_column(col);
        titles.push((col.clone(), *msgid));
    }
    // Secondary columns for the narrow layout: State and User are both shown in
    // the details pane, so hiding them at 1024x600 loses no information.
    let secondary = vec![specs[3].1.clone(), specs[6].1.clone()];

    {
        let state = state.clone();
        selection.connect_selected_item_notify(move |sel| {
            let picked = sel
                .selected_item()
                .and_then(|o| o.downcast::<gtk::glib::BoxedAnyObject>().ok())
                .map(|b| b.borrow::<PortRow>().clone());
            if picked.is_some() {
                *state.selected_port.borrow_mut() = picked;
            }
            crate::ui::details::refresh_selected(&state);
            show_details(&state);
        });
    }

    let status = empty_status();
    let widget = wrap(&view, &status);
    BuiltView {
        widget,
        view,
        titles,
        empty_status: status,
        secondary,
    }
}

// ---------------------------------------------------------------------------
// Processes
// ---------------------------------------------------------------------------

pub fn build_processes_view(state: &Rc<App>) -> BuiltView {
    let selection = gtk::SingleSelection::new(Some(state.stores.processes.clone()));
    // No autoselect (we want an explicit empty state at startup), but clicking a
    // row must keep it selected. `can_unselect(true)` made every click on an
    // already-selected row toggle the details pane off.
    selection.set_autoselect(false);
    selection.set_can_unselect(false);
    let view = gtk::ColumnView::builder()
        .model(&selection)
        .show_row_separators(true)
        .show_column_separators(true)
        .build();

    let specs: Vec<(&'static str, gtk::ColumnViewColumn)> = vec![
        (
            "Name",
            text_col::<ProcessRow>(&tr("Name"), |p| p.name.clone(), 18, true, false),
        ),
        (
            "PID",
            text_col::<ProcessRow>(&tr("PID"), |p| p.pid.to_string(), 7, false, true),
        ),
        (
            "User",
            text_col::<ProcessRow>(
                &tr("User"),
                |p| {
                    p.user
                        .clone()
                        .or_else(|| p.uid.map(|u| u.to_string()))
                        .unwrap_or_else(|| tr("Unknown"))
                },
                10,
                false,
                false,
            ),
        ),
        (
            "Endpoints",
            text_col::<ProcessRow>(
                &tr("Endpoints"),
                |p| p.endpoint_count.to_string(),
                6,
                false,
                true,
            ),
        ),
        (
            "Unit",
            text_col::<ProcessRow>(
                &tr("Unit"),
                |p| {
                    if p.units.is_empty() {
                        tr("No confirmable association")
                    } else {
                        p.units.join(", ")
                    }
                },
                22,
                true,
                true,
            ),
        ),
    ];

    let mut titles = Vec::new();
    for (msgid, col) in &specs {
        view.append_column(col);
        titles.push((col.clone(), *msgid));
    }

    {
        let state = state.clone();
        selection.connect_selected_item_notify(move |sel| {
            let picked = sel
                .selected_item()
                .and_then(|o| o.downcast::<gtk::glib::BoxedAnyObject>().ok())
                .map(|b| b.borrow::<ProcessRow>().clone());
            if picked.is_some() {
                *state.selected_process.borrow_mut() = picked;
            }
            crate::ui::details::refresh_selected(&state);
            show_details(&state);
        });
    }

    let status = adw::StatusPage::builder()
        .icon_name("system-search-symbolic")
        .title(tr("Nothing to show"))
        .build();
    let widget = wrap(&view, &status);
    BuiltView {
        widget,
        view,
        titles,
        empty_status: status,
        // Processes has 5 columns and fits the narrow layout.
        secondary: vec![],
    }
}

// ---------------------------------------------------------------------------
// Services
// ---------------------------------------------------------------------------

pub fn build_services_view(state: &Rc<App>) -> BuiltView {
    let selection = gtk::SingleSelection::new(Some(state.stores.services.clone()));
    // No autoselect (we want an explicit empty state at startup), but clicking a
    // row must keep it selected. `can_unselect(true)` made every click on an
    // already-selected row toggle the details pane off.
    selection.set_autoselect(false);
    selection.set_can_unselect(false);
    let view = gtk::ColumnView::builder()
        .model(&selection)
        .show_row_separators(true)
        .show_column_separators(true)
        .build();

    let specs: Vec<(&'static str, gtk::ColumnViewColumn)> = vec![
        (
            "Unit",
            text_col::<ServiceRow>(&tr("Unit"), |s| s.unit.clone(), 26, true, true),
        ),
        (
            "Scope",
            text_col::<ServiceRow>(
                &tr("Scope"),
                |s| match s.scope {
                    portscope_core::model::UnitScope::System => tr("system"),
                    portscope_core::model::UnitScope::User => tr("user"),
                },
                6,
                false,
                false,
            ),
        ),
        (
            "State",
            text_col::<ServiceRow>(
                &tr("State"),
                |s| tr(s.active_state.as_str()),
                10,
                false,
                false,
            ),
        ),
        (
            "Sub state",
            text_col::<ServiceRow>(&tr("Sub state"), |s| s.sub_state.clone(), 12, false, true),
        ),
        (
            "Main PID",
            text_col::<ServiceRow>(
                &tr("Main PID"),
                |s| {
                    s.main_pid
                        .map(|p| p.to_string())
                        .unwrap_or_else(|| "—".to_string())
                },
                8,
                false,
                true,
            ),
        ),
        (
            "Associated ports",
            text_col::<ServiceRow>(
                &tr("Associated ports"),
                |s| {
                    if s.ports.is_empty() {
                        "—".to_string()
                    } else {
                        s.ports
                            .iter()
                            .map(|(proto, addr, port)| {
                                format!("{} {}:{}", proto.as_str(), addr, port)
                            })
                            .collect::<Vec<_>>()
                            .join(", ")
                    }
                },
                24,
                true,
                true,
            ),
        ),
    ];

    let mut titles = Vec::new();
    for (msgid, col) in &specs {
        view.append_column(col);
        titles.push((col.clone(), *msgid));
    }

    {
        let state = state.clone();
        selection.connect_selected_item_notify(move |sel| {
            let picked = sel
                .selected_item()
                .and_then(|o| o.downcast::<gtk::glib::BoxedAnyObject>().ok())
                .map(|b| b.borrow::<ServiceRow>().clone());
            // `None` here usually means the store is being rebuilt by
            // `refill_all`, not that the user deselected. Overwriting the saved
            // selection in that case made the details pane reset on every
            // auto-refresh. Only an actual item updates the selection.
            if picked.is_some() {
                *state.selected_service.borrow_mut() = picked;
            }
            crate::ui::details::refresh_selected(&state);
            show_details(&state);
        });
    }

    let status = adw::StatusPage::builder()
        .icon_name("system-search-symbolic")
        .title(tr("Nothing to show"))
        .build();
    let widget = wrap(&view, &status);
    BuiltView {
        widget,
        view,
        titles,
        empty_status: status,
        // Sub state and Main PID are secondary; both appear in the details pane.
        secondary: vec![specs[3].1.clone(), specs[4].1.clone()],
    }
}

// ---------------------------------------------------------------------------
// Refill / refilter / retitle
// ---------------------------------------------------------------------------

/// Apply the current search to a row list.
fn visible_ports(rows: &[PortRow], query: &str) -> Vec<PortRow> {
    rows.iter()
        .filter(|r| matches_query(&r.search_text(), query))
        .cloned()
        .collect()
}

fn visible_processes(rows: &[ProcessRow], query: &str) -> Vec<ProcessRow> {
    rows.iter()
        .filter(|r| matches_query(&r.search_text(), query))
        .cloned()
        .collect()
}

fn visible_services(rows: &[ServiceRow], query: &str) -> Vec<ServiceRow> {
    rows.iter()
        .filter(|r| matches_query(&r.search_text(), query))
        .cloned()
        .collect()
}

/// Replace all three list models, honouring the current search text.
///
/// Refilling (rather than mutating) keeps stale rows from lingering, and the
/// selection is re-resolved afterwards from the persisted keys — so a refresh
/// cannot silently move the user's selection to a different object.
pub fn refill_all(state: &Rc<App>) {
    let query = state.search.borrow().clone();
    let (ports, procs, svcs) = {
        let st = state.state.borrow();
        (
            visible_ports(&st.ports, &query),
            visible_processes(&st.processes, &query),
            visible_services(&st.services, &query),
        )
    };

    state.stores.ports.remove_all();
    for r in &ports {
        state.stores.ports.append(&PortObj::new(r.clone()));
    }
    state.stores.processes.remove_all();
    for r in &procs {
        state.stores.processes.append(&ProcObj::new(r.clone()));
    }
    state.stores.services.remove_all();
    for r in &svcs {
        state.stores.services.append(&SvcObj::new(r.clone()));
    }

    restore_selection(state, &ports, &procs, &svcs);
    update_empty_states(state, ports.len(), procs.len(), svcs.len());
}

/// Re-select the previously selected object if it is still present.
fn restore_selection(
    state: &Rc<App>,
    ports: &[PortRow],
    procs: &[ProcessRow],
    svcs: &[ServiceRow],
) {
    // Clone the saved selections out first: holding a `borrow()` while calling
    // `borrow_mut()` on the same RefCell panics, which crashed the app.
    let saved_port = state.selected_port.borrow().clone();
    let saved_proc = state.selected_process.borrow().clone();
    let saved_unit = state.selected_service.borrow().clone();

    if let Some(sel) = saved_port {
        let key = (sel.protocol, sel.local_addr.clone(), sel.port);
        match ports
            .iter()
            .position(|p| (p.protocol, p.local_addr.clone(), p.port) == key)
        {
            Some(idx) => *state.selected_port.borrow_mut() = Some(ports[idx].clone()),
            // The object vanished: clear rather than keep showing stale detail.
            None => *state.selected_port.borrow_mut() = None,
        }
    }
    if let Some(sel) = saved_proc {
        let (pid, ticks) = (sel.pid, sel.start_time_ticks);
        match procs
            .iter()
            .position(|p| p.pid == pid && p.start_time_ticks == ticks)
        {
            Some(idx) => *state.selected_process.borrow_mut() = Some(procs[idx].clone()),
            None => *state.selected_process.borrow_mut() = None,
        }
    }
    if let Some(sel) = saved_unit {
        let unit = sel.unit;
        match svcs.iter().position(|s| s.unit == unit) {
            Some(idx) => *state.selected_service.borrow_mut() = Some(svcs[idx].clone()),
            None => *state.selected_service.borrow_mut() = None,
        }
    }
}

/// Show the appropriate empty page.
///
/// "no data at all" and "search matched nothing" are *different* states and
/// must not share one message (taskbook §5.9, §10), so the title and body are
/// chosen from whether a search is active and whether the underlying snapshot
/// had any rows.
pub fn update_empty_states(state: &Rc<App>, ports: usize, procs: usize, svcs: usize) {
    let searching = !state.search.borrow().trim().is_empty();

    let totals = {
        let st = state.state.borrow();
        (st.ports.len(), st.processes.len(), st.services.len())
    };

    for (built, shown, total) in [
        (state.views.borrow().ports.as_ref(), ports, totals.0),
        (state.views.borrow().processes.as_ref(), procs, totals.1),
        (state.views.borrow().services.as_ref(), svcs, totals.2),
    ] {
        let Some(built) = built else { continue };

        // Swap the visible stack page between table and empty state.
        let Some(stack) = built.widget.downcast_ref::<gtk::Stack>() else {
            continue;
        };

        if shown > 0 {
            stack.set_visible_child_name("table");
            continue;
        }

        let (title, body) = if searching && total > 0 {
            (
                tr("Search returned no results."),
                tr("Try a different search or clear the filters."),
            )
        } else {
            (
                tr("Nothing to show"),
                tr("No endpoints were reported by the system."),
            )
        };
        built.empty_status.set_title(&title);
        built.empty_status.set_description(Some(&body));
        stack.set_visible_child_name("empty");
    }
}

/// Re-apply column titles after a language change.
pub fn rebuild_headers(state: &Rc<App>) {
    for built in [
        state.views.borrow().ports.as_ref(),
        state.views.borrow().processes.as_ref(),
        state.views.borrow().services.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        for (col, msgid) in &built.titles {
            col.set_title(Some(&tr(msgid)));
        }
    }
}
