//! Widget construction and wiring.
//!
//! Design points, each traceable to a taskbook requirement:
//!
//! * **Real data only.** The Spike rendered `SampleData`; this renders whatever
//!   `collect()` returned (Phase 2 gate).
//! * **Background collection.** A [`Collector`] runs off the main thread; results
//!   are applied by polling from the main loop (§7.2).
//! * **Responsive layout (fixes P0-D2).** Below [`NARROW_BREAKPOINT`] the details
//!   pane is a separate page and the table gets the full width, instead of
//!   columns being squeezed out of view.
//! * **Distinct states.** loading / ok / empty / no-match / denied / stale /
//!   failed each render differently (§5.9, §10).

pub mod details;
pub mod settings;
pub mod summary;
pub mod views;

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};

use portscope_core::collect::CollectOptions;
use portscope_core::model::UnitScope;

use crate::i18n::{Lang, LangSetting, Retranslatable, init_gettext, process_env, tr};
use crate::logs::LogSource;
use crate::state::{AppState, EndpointFilter, PortSort, SortDir, View};
use crate::worker::{Collector, Outcome, is_current};
use crate::{APP_ID, APP_VERSION};

/// Below this **window** width the details pane becomes a separate page instead
/// of taking table width.
///
/// The number is derived, not guessed: in the wide layout the details pane takes
/// ~360 px, so the table gets `width - 360`. The Ports table needs roughly
/// 1 040 px to show its six primary columns without clipping. Keeping
/// `width - 360 >= 1 040` gives a breakpoint of ~1 400 px; anything narrower
/// uses the page layout, where the table gets the whole width.
pub const NARROW_BREAKPOINT: i32 = 1400;

/// Minimum width the table is guaranteed in the wide layout.
const MIN_TABLE_WIDTH: i32 = 480;

/// Row newtypes so the GObject list stores are typed.
/// Row storage.
///
/// A `gio::ListStore` needs a real `GObject`. Rather than hand-writing GObject
/// subclasses (which would need `unsafe` or a macro-heavy wrapper for no
/// benefit), rows are stored as `glib::BoxedAnyObject`, which wraps any
/// `'static` Rust value in a `GObject`. Reading a row back is
/// `borrow::<T>()`, and no `unsafe` is involved.
pub mod objects {
    use adw::glib;

    pub type PortObj = glib::BoxedAnyObject;
    pub type ProcObj = glib::BoxedAnyObject;
    pub type SvcObj = glib::BoxedAnyObject;
}

use objects::{PortObj, ProcObj, SvcObj};

// ---------------------------------------------------------------------------
// Shared application state
// ---------------------------------------------------------------------------

pub struct Stores {
    pub ports: gio::ListStore,
    pub processes: gio::ListStore,
    pub services: gio::ListStore,
}

impl Stores {
    fn new() -> Self {
        Self {
            ports: gio::ListStore::new::<PortObj>(),
            processes: gio::ListStore::new::<ProcObj>(),
            services: gio::ListStore::new::<SvcObj>(),
        }
    }
}

/// Handles to the built views, filled in during construction so that refill,
/// refilter and retitle can reach them.
#[derive(Default)]
pub struct Views {
    pub ports: Option<views::BuiltView>,
    pub processes: Option<views::BuiltView>,
    pub services: Option<views::BuiltView>,
}

pub struct App {
    pub state: RefCell<AppState>,
    pub i18n: Retranslatable,
    pub lang_setting: RefCell<LangSetting>,
    pub lang: RefCell<Lang>,
    pub collector: Collector,
    pub logs: LogSource,
    /// Last log outcome per unit, so a refresh does not restart the query.
    pub log_cache: RefCell<std::collections::HashMap<(UnitScope, String), details::LogEntryCache>>,
    /// Generation of the log query currently being polled.
    pub log_generation: RefCell<u64>,
    /// Newest requested collection generation; older results are discarded.
    pub generation: RefCell<u64>,
    pub search: RefCell<String>,
    /// Active endpoint filter and sort, applied when the Ports list is refilled.
    pub port_filter: RefCell<EndpointFilter>,
    pub port_sort: RefCell<(PortSort, SortDir)>,
    pub view: RefCell<View>,
    pub narrow: RefCell<bool>,
    pub refresh_interval: RefCell<Option<u32>>,
    pub stores: Stores,
    pub views: RefCell<Views>,
    /// Selected objects, stored by *value* so a refresh can re-resolve them.
    pub selected_port: RefCell<Option<crate::state::PortRow>>,
    pub selected_process: RefCell<Option<crate::state::ProcessRow>>,
    pub selected_service: RefCell<Option<crate::state::ServiceRow>>,
    pub toast: RefCell<Option<adw::ToastOverlay>>,
    /// The rebuildable inner box of the details pane.
    pub details_content: RefCell<Option<gtk::Box>>,
    /// Filled in `build_ui` so layout switching can move the details widget.
    pub layout: RefCell<Option<Layout>>,
}

/// Handles needed to switch between wide and narrow presentation.
#[derive(Clone)]
pub struct Layout {
    pub paned: gtk::Paned,
    pub stack: gtk::Stack,
    pub details: gtk::Box,
    pub window: adw::ApplicationWindow,
}

impl App {
    fn new() -> Rc<Self> {
        let setting = load_lang_setting().unwrap_or(LangSetting::System);
        let lang = setting.resolve(&process_env());
        Rc::new(Self {
            state: RefCell::new(AppState::default()),
            i18n: Retranslatable::new(),
            lang_setting: RefCell::new(setting),
            lang: RefCell::new(lang),
            collector: Collector::new(),
            logs: LogSource::new(),
            log_cache: RefCell::new(std::collections::HashMap::new()),
            log_generation: RefCell::new(0),
            generation: RefCell::new(0),
            search: RefCell::new(String::new()),
            port_filter: RefCell::new(EndpointFilter::default()),
            port_sort: RefCell::new((PortSort::Port, SortDir::Asc)),
            view: RefCell::new(View::Ports),
            narrow: RefCell::new(false),
            refresh_interval: RefCell::new(Some(5)),
            stores: Stores::new(),
            views: RefCell::new(Views::default()),
            selected_port: RefCell::new(None),
            selected_process: RefCell::new(None),
            selected_service: RefCell::new(None),
            toast: RefCell::new(None),
            details_content: RefCell::new(None),
            layout: RefCell::new(None),
        })
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub fn run() -> glib::ExitCode {
    let setting = load_lang_setting().unwrap_or(LangSetting::System);
    let lang = setting.resolve(&process_env());
    // A missing catalogue must not stop the app: fall back to English and let
    // the UI report it, rather than crashing (taskbook §6.3).
    if lang != Lang::En && init_gettext(lang, localedir().as_deref()).is_err() {
        let _ = init_gettext(Lang::En, localedir().as_deref());
    } else {
        let _ = init_gettext(lang, localedir().as_deref());
    }

    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_startup(|_| load_css());
    app.connect_activate(build_ui);
    app.run()
}

/// Where the compiled `.mo` catalogues live.
fn localedir() -> Option<String> {
    // Installed layout: <prefix>/share/locale
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        // <prefix>/bin/portscope -> <prefix>/share/locale
        let candidate = dir.join("../share/locale");
        if candidate.join("zh_CN/LC_MESSAGES/portscope.mo").exists() {
            return Some(candidate.to_string_lossy().into_owned());
        }
    }
    // Development layout: crates/gtk/po
    let dev = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("po");
    if dev.join("zh_CN/LC_MESSAGES/portscope.mo").exists() {
        return Some(dev.to_string_lossy().into_owned());
    }
    None
}

fn load_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(
        "
        .mono { font-family: monospace; }
        .dim { opacity: 0.65; }
        .stale { font-weight: bold; }
        .evidence { font-family: monospace; font-size: 0.85em; }
        .fault { font-weight: bold; }
        .field-label { font-weight: bold; }
        ",
    );
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

// ---------------------------------------------------------------------------
// Settings persistence (language only; no snapshots are ever stored)
// ---------------------------------------------------------------------------

fn settings_path() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config"))
        })?;
    Some(base.join("portscope").join("settings.ini"))
}

pub fn load_lang_setting() -> Option<LangSetting> {
    let path = settings_path()?;
    let text = std::fs::read_to_string(path).ok()?;
    text.lines()
        .find_map(|l| l.strip_prefix("language=").map(LangSetting::parse))
}

pub fn save_lang_setting(setting: LangSetting) -> Result<(), String> {
    let path = settings_path().ok_or("no config directory")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, format!("language={}\n", setting.key())).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// UI construction
// ---------------------------------------------------------------------------

fn build_ui(app: &adw::Application) {
    let state = App::new();

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title(tr("PortScope"))
        .default_width(1280)
        .default_height(800)
        // Minimum must not forbid 1024×600 CSS (Spike defect B3 / P0-D3):
        // keep both axes well below the compact target.
        .width_request(360)
        .height_request(240)
        .build();

    let toast_overlay = adw::ToastOverlay::new();
    *state.toast.borrow_mut() = Some(toast_overlay.clone());

    let header = adw::HeaderBar::new();

    let search_entry = gtk::SearchEntry::builder()
        .placeholder_text(tr("Search ports, processes, services…"))
        .hexpand(true)
        .build();

    let refresh_btn = gtk::Button::builder()
        .icon_name("view-refresh-symbolic")
        .tooltip_text(tr("Refresh"))
        .build();

    let menu_btn = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .tooltip_text(tr("Menu"))
        .build();

    header.pack_start(&search_entry);
    header.pack_end(&menu_btn);
    header.pack_end(&refresh_btn);

    // --- permission banner -------------------------------------------------
    let banner = adw::Banner::new("");
    banner.set_revealed(false);

    // --- three views -------------------------------------------------------
    let ports_built = views::build_ports_view(&state);
    let processes_built = views::build_processes_view(&state);
    let services_built = views::build_services_view(&state);
    let ports_page = ports_built.widget.clone();
    let processes_page = processes_built.widget.clone();
    let services_page = services_built.widget.clone();

    let stack = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .vexpand(true)
        .hexpand(true)
        .build();
    stack.add_titled(&ports_page, Some(View::Ports.id()), &tr("Ports"));
    stack.add_titled(
        &processes_page,
        Some(View::Processes.id()),
        &tr("Processes"),
    );
    stack.add_titled(&services_page, Some(View::Services.id()), &tr("Services"));

    let switcher = gtk::StackSwitcher::builder()
        .stack(&stack)
        .halign(gtk::Align::Center)
        .build();
    header.set_title_widget(Some(&switcher));

    {
        let mut v = state.views.borrow_mut();
        v.ports = Some(ports_built);
        v.processes = Some(processes_built);
        v.services = Some(services_built);
    }

    // --- details -----------------------------------------------------------
    let details_box = details::build_details_box(&state);

    // Narrow mode wraps the list stack and details into one stack.
    let content_stack = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::SlideLeft)
        .vexpand(true)
        .hexpand(true)
        .build();
    content_stack.add_named(&stack, Some("list"));

    let paned = gtk::Paned::builder()
        .orientation(gtk::Orientation::Horizontal)
        .wide_handle(true)
        .vexpand(true)
        .hexpand(true)
        .build();
    paned.set_start_child(Some(&content_stack));
    paned.set_resize_start_child(true);
    paned.set_shrink_start_child(false);
    paned.set_position(820);

    // Wide layout by default: details beside the table.
    paned.set_end_child(Some(&details_box));

    let status_label = gtk::Label::builder()
        .xalign(0.0)
        .margin_start(12)
        .margin_end(12)
        .margin_top(6)
        .margin_bottom(6)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    status_label.add_css_class("dim");

    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.append(&banner);
    body.append(&paned);
    body.append(&status_label);

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.append(&header);
    root.append(&body);

    toast_overlay.set_child(Some(&root));
    window.set_content(Some(&toast_overlay));

    *state.layout.borrow_mut() = Some(Layout {
        paned: paned.clone(),
        stack: content_stack.clone(),
        details: details_box.clone(),
        window: window.clone(),
    });

    // --- menu --------------------------------------------------------------
    let menu = build_menu_model(&state);
    menu_btn.set_menu_model(Some(&menu));

    // --- wiring ------------------------------------------------------------
    {
        let state = state.clone();
        search_entry.connect_search_changed(move |e| {
            *state.search.borrow_mut() = e.text().to_string();
            views::refill_all(&state);
            details::refresh_selected(&state);
        });
    }
    {
        let state = state.clone();
        let content_stack = content_stack.clone();
        refresh_btn.connect_clicked(move |_| {
            content_stack.set_visible_child_name("list");
            start_collection(&state);
        });
    }
    {
        let state = state.clone();
        stack.connect_visible_child_name_notify(move |s| {
            let name = s.visible_child_name().unwrap_or_default();
            *state.view.borrow_mut() = match name.as_str() {
                "processes" => View::Processes,
                "services" => View::Services,
                _ => View::Ports,
            };
            details::refresh_selected(&state);
        });
    }
    {
        let state = state.clone();
        window.connect_close_request(move |_| {
            state.collector.cancel_current();
            state.logs.cancel_current();
            glib::Propagation::Proceed
        });
    }

    // Keyboard: Ctrl+F focuses search, Ctrl+R refreshes, Esc leaves details.
    {
        let content_stack = content_stack.clone();
        let search_entry = search_entry.clone();
        let state_k = state.clone();
        let key = gtk::EventControllerKey::new();
        key.connect_key_pressed(move |_, keyval, _, modifier| {
            use gtk::gdk::{Key, ModifierType};
            let ctrl = modifier.contains(ModifierType::CONTROL_MASK);
            if ctrl && (keyval == Key::f || keyval == Key::F) {
                search_entry.grab_focus();
                return glib::Propagation::Stop;
            }
            if ctrl && (keyval == Key::r || keyval == Key::R) {
                start_collection(&state_k);
                return glib::Propagation::Stop;
            }
            if keyval == Key::Escape
                && content_stack.visible_child_name().as_deref() == Some("details")
            {
                content_stack.set_visible_child_name("list");
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        window.add_controller(key);
    }

    // Responsive layout.
    wire_responsive(&state);

    // Re-apply translated text on every language change.
    install_retranslations(
        &state,
        &search_entry,
        &stack,
        &refresh_btn,
        &banner,
        &status_label,
    );

    // Auto refresh.
    install_auto_refresh(&state);

    start_collection(&state);
    window.present();
}

fn build_menu_model(state: &Rc<App>) -> gio::Menu {
    let menu = gio::Menu::new();

    let lang_menu = gio::Menu::new();
    lang_menu.append(Some(&tr("System default")), Some("app.lang-system"));
    lang_menu.append(Some(Lang::En.endonym()), Some("app.lang-en"));
    lang_menu.append(Some(Lang::ZhCn.endonym()), Some("app.lang-zh"));
    menu.append_submenu(Some(&tr("Language")), &lang_menu);

    let refresh_menu = gio::Menu::new();
    refresh_menu.append(Some(&tr("Off")), Some("app.refresh-off"));
    refresh_menu.append(Some(&tr("5 seconds")), Some("app.refresh-5"));
    refresh_menu.append(Some(&tr("10 seconds")), Some("app.refresh-10"));
    refresh_menu.append(Some(&tr("30 seconds")), Some("app.refresh-30"));
    menu.append_submenu(Some(&tr("Auto-refresh")), &refresh_menu);

    menu.append(
        Some(&tr("Copy diagnostic summary")),
        Some("app.copy-summary"),
    );
    menu.append(Some(&tr("Settings")), Some("app.settings"));
    menu.append(Some(&tr("About PortScope")), Some("app.about"));

    install_actions(state);
    menu
}

fn install_actions(state: &Rc<App>) {
    let Some(app) = gio::Application::default().and_then(|a| a.downcast::<adw::Application>().ok())
    else {
        return;
    };

    for (name, setting) in [
        ("lang-system", LangSetting::System),
        ("lang-en", LangSetting::Fixed(Lang::En)),
        ("lang-zh", LangSetting::Fixed(Lang::ZhCn)),
    ] {
        let action = gio::SimpleAction::new(name, None);
        let state = state.clone();
        action.connect_activate(move |_, _| set_language(&state, setting));
        app.add_action(&action);
    }

    for (name, choice) in [
        ("refresh-off", None),
        ("refresh-5", Some(5u32)),
        ("refresh-10", Some(10u32)),
        ("refresh-30", Some(30u32)),
    ] {
        let action = gio::SimpleAction::new(name, None);
        let state = state.clone();
        action.connect_activate(move |_, _| {
            *state.refresh_interval.borrow_mut() = choice;
        });
        app.add_action(&action);
    }

    let copy = gio::SimpleAction::new("copy-summary", None);
    let state_c = state.clone();
    copy.connect_activate(move |_, _| {
        details::copy_diagnostic_summary(&state_c);
    });
    app.add_action(&copy);

    let settings = gio::SimpleAction::new("settings", None);
    let state_s = state.clone();
    settings.connect_activate(move |_, _| settings::present(&state_s));
    app.add_action(&settings);

    let about = gio::SimpleAction::new("about", None);
    let state_a = state.clone();
    about.connect_activate(move |_, _| {
        let dialog = adw::AboutWindow::builder()
            .application_name("PortScope")
            .application_icon(APP_ID)
            .version(APP_VERSION)
            .developer_name("PortScope contributors")
            .comments(tr("A local port, process and service inspector."))
            .license_type(gtk::License::MitX11)
            .build();
        if let Some(layout) = state_a.layout.borrow().as_ref() {
            dialog.set_transient_for(Some(&layout.window));
        }
        dialog.present();
    });
    app.add_action(&about);
}

// ---------------------------------------------------------------------------
// Language switching
// ---------------------------------------------------------------------------

/// Public entry so the settings window and the menu share one implementation.
pub fn apply_language_setting(state: &Rc<App>, setting: LangSetting) {
    set_language(state, setting)
}

fn set_language(state: &Rc<App>, setting: LangSetting) {
    let lang = setting.resolve(&process_env());
    if init_gettext(lang, localedir().as_deref()).is_err() {
        // Fall back to English and tell the user, instead of leaving the UI in a
        // half-translated state (taskbook §6.3).
        let _ = init_gettext(Lang::En, localedir().as_deref());
        if let Some(toast) = state.toast.borrow().as_ref() {
            toast.add_toast(adw::Toast::new("Language unavailable; using English."));
        }
    }
    *state.lang.borrow_mut() = lang;
    *state.lang_setting.borrow_mut() = setting;

    // Persist; a failure is reported rather than silently ignored.
    if let Err(e) = save_lang_setting(setting)
        && let Some(toast) = state.toast.borrow().as_ref()
    {
        toast.add_toast(adw::Toast::new(&format!(
            "Could not save language preference: {e}"
        )));
    }

    // Re-apply every registered string. Selection and filters live in the GTK
    // models, so they survive without being saved and restored.
    state.i18n.apply_all();
    views::rebuild_headers(state);
    views::refill_all(state);
    details::refresh_selected(state);
}

fn install_retranslations(
    state: &Rc<App>,
    search_entry: &gtk::SearchEntry,
    stack: &gtk::Stack,
    refresh_btn: &gtk::Button,
    banner: &adw::Banner,
    status_label: &gtk::Label,
) {
    {
        let search_entry = search_entry.clone();
        state.i18n.register(move || {
            search_entry.set_placeholder_text(Some(&tr("Search ports, processes, services…")));
        });
    }
    {
        let refresh_btn = refresh_btn.clone();
        state.i18n.register(move || {
            refresh_btn.set_tooltip_text(Some(&tr("Refresh")));
        });
    }
    {
        let stack = stack.clone();
        state.i18n.register(move || {
            if let Some(page) = stack.child_by_name(View::Ports.id()) {
                stack.page(&page).set_title(&tr("Ports"));
            }
            if let Some(page) = stack.child_by_name(View::Processes.id()) {
                stack.page(&page).set_title(&tr("Processes"));
            }
            if let Some(page) = stack.child_by_name(View::Services.id()) {
                stack.page(&page).set_title(&tr("Services"));
            }
        });
    }
    {
        // Clone the registry separately from `state`: the closure needs to own
        // `state`, and it cannot borrow the same field it is being registered on.
        let registry = state.i18n.clone();
        let state = state.clone();
        let banner = banner.clone();
        registry.register(move || update_banner(&state, &banner));
    }
    {
        let registry = state.i18n.clone();
        let state = state.clone();
        let status_label = status_label.clone();
        registry.register(move || update_status(&state, &status_label));
    }
}

// ---------------------------------------------------------------------------
// Status bar and banner text
// ---------------------------------------------------------------------------

fn update_status(state: &Rc<App>, label: &gtk::Label) {
    use crate::state::{Freshness, LoadState};

    let st = state.state.borrow();
    let counts = format!(
        "{} {} · {} {} · {} {}",
        st.ports.len(),
        tr("Ports"),
        st.processes.len(),
        tr("Processes"),
        st.services.len(),
        tr("Services")
    );

    // Lifecycle first, then freshness, then the permission caveat: the most
    // important fact should be readable without scanning the whole line.
    let mut line = match &st.load {
        LoadState::Loading => format!("… {}", tr("Refreshing")),
        LoadState::Cancelled => tr("Refresh cancelled"),
        LoadState::Failed(_) => tr("Collection failed"),
        _ => counts.clone(),
    };

    if st.freshness == Freshness::Stale {
        line = format!("{} · {}", tr("Data may be stale"), line);
    }
    if st.unowned_endpoint_count() > 0 {
        line = format!(
            "{} · {} ({})",
            line,
            tr("Permission denied"),
            st.unowned_endpoint_count()
        );
    }
    if let LoadState::Failed(msg) = &st.load {
        line = format!("{} — {}", line, msg);
    }
    label.set_text(&line);
}

fn update_banner(state: &Rc<App>, banner: &adw::Banner) {
    let st = state.state.borrow();
    let has_perm = !st.permission_errors().is_empty();
    if has_perm {
        banner.set_title(&tr("Some information is unavailable"));
        banner.set_revealed(true);
    } else {
        banner.set_revealed(false);
    }
}

// ---------------------------------------------------------------------------
// Collection
// ---------------------------------------------------------------------------

/// Trigger a collection now (used by the Refresh button and settings).
pub fn refresh_now(state: &Rc<App>) {
    start_collection(state);
}

pub fn start_collection(state: &Rc<App>) {
    state.state.borrow_mut().begin_loading();
    // Reflect the loading state immediately, without waiting for the worker.
    state.i18n.apply_all();

    trace("collection started");
    let opts = CollectOptions::default();
    let (generation, rx) = state.collector.start(opts);
    *state.generation.borrow_mut() = generation;

    let state_poll = state.clone();
    glib::timeout_add_local(std::time::Duration::from_millis(50), move || {
        // Non-blocking receive: keeps the main loop free.
        match rx.try_recv() {
            Ok(result) => {
                let current = *state_poll.generation.borrow();
                if !is_current(result.generation, current) {
                    // A newer collection superseded this one.
                    return glib::ControlFlow::Break;
                }
                apply_result(&state_poll, result.outcome);
                glib::ControlFlow::Break
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
        }
    });
}

/// Optional state tracing, enabled with `PORTSCOPE_TRACE=1`.
///
/// Used to produce evidence for the collection lifecycle without a debugger.
fn trace(msg: &str) {
    if std::env::var_os("PORTSCOPE_TRACE").is_some() {
        eprintln!("[portscope] {msg}");
    }
}

fn apply_result(state: &Rc<App>, outcome: Outcome) {
    match outcome {
        Outcome::Snapshot(snap) => {
            let n = snap.endpoints.len();
            let errs = snap.errors.len();
            state.state.borrow_mut().apply_snapshot(*snap);
            views::refill_all(state);
            trace(&format!(
                "applied snapshot: {n} endpoints, {errs} errors -> Ok"
            ));
        }
        Outcome::Cancelled => {
            state.state.borrow_mut().apply_cancelled();
            trace("collection cancelled");
        }
        Outcome::Failed(msg) => {
            trace(&format!("collection failed: {msg}"));
            state.state.borrow_mut().apply_failure(msg);
        }
    }
    // Re-render dependent widgets.
    if let Some(layout) = state.layout.borrow().as_ref() {
        let banner = find_banner(&layout.window);
        if let Some(b) = banner {
            update_banner(state, &b);
        }
    }
    state.i18n.apply_all();
    details::refresh_selected(state);
}

/// Walk the widget tree to find the banner (only used to refresh its text).
fn find_banner(window: &adw::ApplicationWindow) -> Option<adw::Banner> {
    fn walk(w: &gtk::Widget, found: &mut Option<adw::Banner>) {
        if found.is_some() {
            return;
        }
        if let Some(b) = w.downcast_ref::<adw::Banner>() {
            *found = Some(b.clone());
            return;
        }
        let mut child = w.first_child();
        while let Some(c) = child {
            walk(&c, found);
            child = c.next_sibling();
        }
    }
    let mut found = None;
    if let Some(content) = window.content() {
        walk(&content, &mut found);
    }
    found
}

fn install_auto_refresh(state: &Rc<App>) {
    let state = state.clone();
    // Tick once per second; refresh when the interval has elapsed.
    let mut elapsed: u32 = 0;
    glib::timeout_add_local(std::time::Duration::from_secs(1), move || {
        let interval = *state.refresh_interval.borrow();
        match interval {
            None => {
                elapsed = 0;
            }
            Some(secs) => {
                elapsed += 1;
                if elapsed >= secs {
                    elapsed = 0;
                    // Skip if a collection is already in flight.
                    if !matches!(state.state.borrow().load, crate::state::LoadState::Loading) {
                        start_collection(&state);
                    }
                }
            }
        }
        glib::ControlFlow::Continue
    });
}

// ---------------------------------------------------------------------------
// Responsive layout (fixes P0-D2)
// ---------------------------------------------------------------------------

fn wire_responsive(state: &Rc<App>) {
    let Some(layout) = state.layout.borrow().clone() else {
        return;
    };

    let state_w = state.clone();
    let layout_w = layout.clone();
    // Fires on every width change, including the first allocation.
    layout.paned.add_tick_callback(move |paned, _clock| {
        let width = paned.width();
        if width <= 0 {
            return glib::ControlFlow::Continue;
        }
        let want_narrow = width < NARROW_BREAKPOINT;
        if want_narrow == *state_w.narrow.borrow() {
            return glib::ControlFlow::Continue;
        }
        *state_w.narrow.borrow_mut() = want_narrow;

        if want_narrow {
            // Move the details pane out of the split and make it a page, so the
            // table keeps the full width. This is the actual P0-D2 fix.
            layout_w.paned.set_end_child(None::<&gtk::Widget>);
            if layout_w.stack.child_by_name("details").is_none() {
                layout_w.stack.add_named(&layout_w.details, Some("details"));
            }
            layout_w.stack.set_visible_child_name("list");
            // Drop the secondary columns so the remaining ones stay readable,
            // instead of squeezing all of them into "…".
            set_secondary_columns(&state_w, false);
        } else {
            // Put the details pane back beside the table.
            if let Some(page) = layout_w.stack.child_by_name("details") {
                layout_w.stack.remove(&page);
            }
            layout_w.paned.set_end_child(Some(&layout_w.details));
            let total = layout_w.paned.width();
            let pos = total.saturating_sub(360).max(MIN_TABLE_WIDTH);
            layout_w.paned.set_position(pos);
            set_secondary_columns(&state_w, true);
        }
        glib::ControlFlow::Continue
    });

    // Never let the pane crush the table.
    layout.paned.connect_position_notify(|p| {
        let total = p.width();
        if total <= 0 {
            return;
        }
        let min_table = MIN_TABLE_WIDTH;
        if total - p.position() < min_table {
            let want = (total - min_table).max(min_table / 2);
            if want != p.position() {
                p.set_position(want);
            }
        }
    });
}

/// Show or hide the secondary table columns across all three views.
fn set_secondary_columns(state: &Rc<App>, visible: bool) {
    let views = state.views.borrow();
    for built in [
        views.ports.as_ref(),
        views.processes.as_ref(),
        views.services.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        built.set_secondary_visible(visible);
    }
}

// ---------------------------------------------------------------------------
// Public helper used by the details pane to open the details page
// ---------------------------------------------------------------------------

pub fn show_details(state: &Rc<App>) {
    if *state.narrow.borrow()
        && let Some(layout) = state.layout.borrow().as_ref()
    {
        layout.stack.set_visible_child_name("details");
    }
}
