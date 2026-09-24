//! Application: single instance, command-line opening, accelerators, style,
//! preferences / about / shortcuts / properties dialogs.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use adw::prelude::*;
use gtk::gdk;
use gtk::gio;
use gtk::glib;

use crate::config::{APP_ID, APP_NAME, VERSION};
use crate::i18n::tr;
use crate::settings::settings;
use crate::window::Window;

const ACCELS: &[(&str, &[&str])] = &[
    ("win.new-tab", &["<Ctrl>t"]),
    ("win.close-tab", &["<Ctrl>w"]),
    ("win.back", &["<Alt>Left"]),
    ("win.forward", &["<Alt>Right"]),
    ("win.up", &["<Alt>Up"]),
    ("win.favourite", &["<Ctrl>d"]),
    ("win.location", &["<Ctrl>l"]),
    ("win.view-mode::grid", &["<Ctrl>1"]),
    ("win.view-mode::waterfall", &["<Ctrl>2"]),
    ("win.view-mode::list", &["<Ctrl>3"]),
    ("win.zoom-in", &["<Ctrl>plus", "<Ctrl>equal", "<Ctrl>KP_Add"]),
    ("win.zoom-out", &["<Ctrl>minus", "<Ctrl>KP_Subtract"]),
    ("win.show-hidden", &["<Ctrl>h"]),
    ("win.refresh", &["F5", "<Ctrl>r"]),
    ("win.filter", &["<Ctrl>f"]),
    ("win.fullscreen", &["F11"]),
    ("win.toggle-sidebar", &["F9"]),
    ("win.new-folder", &["<Ctrl><Shift>n"]),
    ("win.copy-path", &["<Ctrl><Shift>c"]),
    ("win.rename", &["F2"]),
    ("win.properties", &["<Alt>Return"]),
    ("win.recursive", &["<Ctrl><Shift>r"]),
    ("app.preferences", &["<Ctrl>comma"]),
    ("app.shortcuts", &["<Ctrl>question"]),
    ("app.quit", &["<Ctrl>q"]),
];

/// Keys that text entries need (Delete, Ctrl+C/X/V, …) are *not* global
/// accelerators; views handle them so they never steal from entries.
fn view_only_shortcut(key: gdk::Key, state: gdk::ModifierType) -> Option<&'static str> {
    use gdk::Key;
    let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
    let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
    match key {
        Key::Delete | Key::KP_Delete if shift => Some("win.delete-permanently"),
        Key::Delete | Key::KP_Delete => Some("win.trash"),
        Key::c | Key::C if ctrl && !shift => Some("win.copy"),
        Key::x | Key::X if ctrl => Some("win.cut"),
        Key::v | Key::V if ctrl => Some("win.paste"),
        _ => None,
    }
}

pub struct App {
    pub app: adw::Application,
    window: RefCell<Option<Window>>,
}

pub fn run() -> glib::ExitCode {
    let smoke = std::env::var_os("FLOWLIN_SMOKE_TEST").is_some();
    let mut flags = gio::ApplicationFlags::HANDLES_OPEN;
    if smoke {
        flags |= gio::ApplicationFlags::NON_UNIQUE;
    }
    let app = adw::Application::builder().application_id(APP_ID).flags(flags).build();
    let state = Rc::new(App { app: app.clone(), window: RefCell::new(None) });

    app.connect_startup(|app| {
        crate::fs::formats::init();
        // Never record opened files in the recent-files list.
        if let Some(s) = gtk::Settings::default() {
            s.set_gtk_recent_files_enabled(false);
        }
        load_css();
        apply_color_scheme();
        settings().connect_changed(Some("color-scheme"), |_, _| apply_color_scheme());
        for (action, accels) in ACCELS {
            app.set_accels_for_action(action, accels);
        }
        setup_app_actions(app);
    });

    let st = state.clone();
    app.connect_activate(move |_| {
        let w = st.window();
        // First launch opens Home; a second launch (single instance) adds a tab.
        w.open_tab(&glib::home_dir());
        w.present();
    });

    let st = state.clone();
    app.connect_open(move |_, files, _| {
        let w = st.window();
        for f in files {
            if let Some(p) = f.path() {
                w.open_path(&p);
            }
        }
        if w.active_tab().is_none() {
            w.open_tab(&glib::home_dir());
        }
        w.present();
        if smoke {
            crate::app::smoke_test(w.clone(), st.app.clone());
        }
    });

    app.run()
}

impl App {
    fn window(&self) -> Window {
        if let Some(w) = self.window.borrow().as_ref() {
            if w.window().is_visible() || w.active_tab().is_some() {
                return w.clone();
            }
        }
        let w = Window::new(&self.app);
        install_view_shortcuts(&w);
        install_frame_logger(&w);
        *self.window.borrow_mut() = Some(w.clone());
        w
    }
}

/// `FLOWLIN_FRAME_LOG=1`: log frames that took longer than 24 ms.
fn install_frame_logger(w: &Window) {
    if std::env::var_os("FLOWLIN_FRAME_LOG").is_none() {
        return;
    }
    let last = std::cell::Cell::new(0i64);
    w.window().add_tick_callback(move |_, clock| {
        let t = clock.frame_time();
        let prev = last.replace(t);
        let dt = (t - prev) as f64 / 1000.0;
        if prev != 0 && dt > 24.0 && dt < 2000.0 {
            tracing::warn!("slow frame: {dt:.1} ms");
        }
        glib::ControlFlow::Continue
    });
}

fn install_view_shortcuts(w: &Window) {
    let keys = gtk::EventControllerKey::new();
    let win = w.window().clone();
    let window = w.clone();
    keys.connect_key_pressed(move |_, key, _, state| {
        // Only when the focus is not inside a text entry.
        if crate::tab::text_entry_has_focus(&win) {
            return glib::Propagation::Proceed;
        }
        // Folder keys (W/A/S/D/R) also work when the sidebar has focus.
        let plain = !state
            .intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK | gdk::ModifierType::SUPER_MASK);
        if plain && matches!(key.to_lower(), gdk::Key::w | gdk::Key::a | gdk::Key::s | gdk::Key::d | gdk::Key::r) {
            if let Some(t) = window.active_tab().filter(|t| !t.viewer_open()) {
                return t.handle_view_key(key, state);
            }
        }
        let Some(action) = view_only_shortcut(key, state) else { return glib::Propagation::Proceed };
        let name = action.trim_start_matches("win.");
        let _ = WidgetExt::activate_action(&win, &format!("win.{name}"), None);
        glib::Propagation::Stop
    });
    w.window().add_controller(keys);
}

fn load_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(include_str!("../data/style.css"));
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    }
}

fn apply_color_scheme() {
    let scheme = match settings().string("color-scheme").as_str() {
        "light" => adw::ColorScheme::ForceLight,
        "dark" => adw::ColorScheme::ForceDark,
        _ => adw::ColorScheme::PreferLight,
    };
    adw::StyleManager::default().set_color_scheme(scheme);
}

fn setup_app_actions(app: &adw::Application) {
    let quit = gio::SimpleAction::new("quit", None);
    let a = app.clone();
    quit.connect_activate(move |_, _| {
        for w in a.windows() {
            w.close();
        }
    });
    app.add_action(&quit);

    let about = gio::SimpleAction::new("about", None);
    let a = app.clone();
    about.connect_activate(move |_, _| {
        let d = adw::AboutDialog::builder()
            .application_name(APP_NAME)
            .application_icon(APP_ID)
            .version(VERSION)
            .developer_name("Flowlin contributors")
            .license_type(gtk::License::Gpl30)
            .comments(tr("A fast local image browser, modelled on FlowVision. No network access, no tracking, nothing remembered on disk."))
            .website("https://flowvision.app")
            .build();
        d.present(a.active_window().as_ref());
    });
    app.add_action(&about);

    let prefs = gio::SimpleAction::new("preferences", None);
    let a = app.clone();
    prefs.connect_activate(move |_, _| show_preferences(a.active_window().as_ref()));
    app.add_action(&prefs);

    let shortcuts = gio::SimpleAction::new("shortcuts", None);
    let a = app.clone();
    shortcuts.connect_activate(move |_, _| show_shortcuts(a.active_window().as_ref()));
    app.add_action(&shortcuts);
}

fn show_preferences(parent: Option<&gtk::Window>) {
    let s = settings();
    let dialog = adw::PreferencesDialog::new();
    let page = adw::PreferencesPage::new();

    let look = adw::PreferencesGroup::new();
    look.set_title(&tr("Appearance"));
    let scheme = adw::ComboRow::new();
    scheme.set_title(&tr("Style"));
    let ids = ["follow", "light", "dark"];
    scheme.set_model(Some(&gtk::StringList::new(&[&tr("Follow System"), &tr("Light"), &tr("Dark")])));
    scheme.set_selected(ids.iter().position(|i| *i == s.string("color-scheme")).unwrap_or(0) as u32);
    scheme.connect_selected_notify(move |r| {
        let _ = settings().set_string("color-scheme", ids[r.selected() as usize % 3]);
    });
    look.add(&scheme);
    for (key, title) in
        [("show-names", tr("Show File Names")), ("show-folder-labels", tr("Show Folder Under Name in Recursive Mode"))]
    {
        let row = adw::SwitchRow::new();
        row.set_title(&title);
        s.bind(key, &row, "active").build();
        look.add(&row);
    }
    page.add(&look);

    let browse = adw::PreferencesGroup::new();
    browse.set_title(&tr("Browsing"));
    let nav = adw::SwitchRow::new();
    nav.set_title(&tr("Single-Key Folder Navigation"));
    nav.set_subtitle(&tr("W parent, S back, A/D previous/next folder with images, R recursive. When off, letters only jump to matching file names."));
    s.bind("single-key-navigation", &nav, "active").build();
    browse.add(&nav);
    let hidden = adw::SwitchRow::new();
    hidden.set_title(&tr("Show Hidden Files"));
    s.bind("show-hidden", &hidden, "active").build();
    browse.add(&hidden);
    let same = adw::SwitchRow::new();
    same.set_title(&tr("Recursive Mode Stays on One File System"));
    s.bind("same-device", &same, "active").build();
    browse.add(&same);
    let cap = adw::SpinRow::with_range(1000.0, 10_000_000.0, 1000.0);
    cap.set_title(&tr("Recursive File Limit"));
    s.bind("recursive-cap", &cap, "value").build();
    browse.add(&cap);
    page.add(&browse);

    let viewer = adw::PreferencesGroup::new();
    viewer.set_title(&tr("Viewer"));
    let interval = adw::SpinRow::with_range(1.0, 600.0, 1.0);
    interval.set_title(&tr("Slideshow Interval (Seconds)"));
    s.bind("slideshow-interval", &interval, "value").build();
    viewer.add(&interval);
    page.add(&viewer);

    let mem = adw::PreferencesGroup::new();
    mem.set_title(&tr("Privacy & Memory"));
    mem.set_description(Some(&tr(
        "Thumbnails are kept in memory only and never written to disk. Flowlin does not remember which folders or files you opened.",
    )));
    let cache = adw::SpinRow::with_range(32.0, 4096.0, 32.0);
    cache.set_title(&tr("Thumbnail Memory (MB)"));
    s.bind("thumbnail-memory", &cache, "value").build();
    mem.add(&cache);
    page.add(&mem);

    dialog.add(&page);
    dialog.present(parent);
}

fn show_shortcuts(parent: Option<&gtk::Window>) {
    let groups: &[(&str, &[(&str, &str)])] = &[
        (
            "Folders",
            &[
                ("Parent folder", "<Alt>Up BackSpace w"),
                ("Back / forward", "<Alt>Left <Alt>Right"),
                ("Previously visited folder", "s"),
                ("Previous / next folder with images", "a d"),
                ("Include subfolders (recursive)", "r <Ctrl><Shift>r"),
                ("Edit location", "<Ctrl>l"),
                ("Toggle favourite", "<Ctrl>d"),
                ("Refresh", "F5"),
                ("Show hidden files", "<Ctrl>h"),
            ],
        ),
        (
            "Thumbnails",
            &[
                ("Open viewer", "Return space"),
                ("Grid / waterfall / list", "<Ctrl>1 <Ctrl>2 <Ctrl>3"),
                ("Larger / smaller thumbnails", "<Ctrl>plus <Ctrl>minus"),
                ("Filter by name", "<Ctrl>f"),
                ("Select all", "<Ctrl>a"),
                ("Rate 1–5 stars / clear", "1 5 0"),
            ],
        ),
        (
            "Files",
            &[
                ("Copy / cut / paste", "<Ctrl>c <Ctrl>x <Ctrl>v"),
                ("Copy path", "<Ctrl><Shift>c"),
                ("Rename", "F2"),
                ("Move to trash", "Delete"),
                ("Delete permanently", "<Shift>Delete"),
                ("New folder", "<Ctrl><Shift>n"),
                ("Properties", "<Alt>Return"),
            ],
        ),
        (
            "Viewer",
            &[
                ("Previous / next image", "Left Right a d"),
                ("Row above / below (as in the grid)", "Up Down w s"),
                ("Close viewer", "Escape Return space"),
                ("Zoom in / out", "plus minus"),
                ("100 % / fit / fill", "1 0 <Shift>f"),
                ("Rotate (view only)", "r <Shift>r"),
                ("Slideshow", "p"),
                ("Show info", "i"),
                ("Rate 2–5 stars (Shift+1 for 1, Shift+0 clears)", "2 5"),
                ("Move to trash", "Delete"),
                ("Full screen", "F11"),
            ],
        ),
        (
            "Video",
            &[
                ("Play / pause", "k"),
                ("Seek 5 s / 10 s", "Left Right <Shift>Left <Shift>Right"),
                ("Volume up / down", "Up Down"),
                ("Mute", "m"),
                ("Previous / next file", "Page_Up Page_Down"),
            ],
        ),
        (
            "Window",
            &[
                ("New tab / close tab", "<Ctrl>t <Ctrl>w"),
                ("Toggle sidebar", "F9"),
                ("Preferences", "<Ctrl>comma"),
                ("Keyboard shortcuts", "<Ctrl>question"),
                ("Quit", "<Ctrl>q"),
            ],
        ),
    ];
    let mut xml = String::from(
        r#"<interface><object class="GtkShortcutsWindow" id="w"><property name="modal">1</property><child><object class="GtkShortcutsSection"><property name="section-name">main</property><property name="max-height">12</property>"#,
    );
    for (title, items) in groups {
        xml.push_str(&format!(
            r#"<child><object class="GtkShortcutsGroup"><property name="title">{}</property>"#,
            glib::markup_escape_text(&tr(title))
        ));
        for (t, accel) in *items {
            xml.push_str(&format!(
                r#"<child><object class="GtkShortcutsShortcut"><property name="title">{}</property><property name="accelerator">{}</property></object></child>"#,
                glib::markup_escape_text(&tr(t)),
                glib::markup_escape_text(accel)
            ));
        }
        xml.push_str("</object></child>");
    }
    xml.push_str("</object></child></object></interface>");
    let builder = gtk::Builder::from_string(&xml);
    #[allow(deprecated)]
    let w: gtk::ShortcutsWindow = builder.object("w").unwrap();
    #[allow(deprecated)]
    {
        w.set_transient_for(parent);
        w.present();
    }
}

/// Everything the properties dialog shows, gathered on a worker thread.
struct PropertyRows {
    file: Vec<(&'static str, String)>,
    image: Vec<(&'static str, String)>,
    exif: Vec<(&'static str, String)>,
}

fn gather_properties(path: &Path) -> PropertyRows {
    let mut file = vec![
        ("Name", crate::util::file_name(path)),
        ("Folder", path.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()),
    ];
    let md = std::fs::metadata(path).ok();
    if let Some(md) = &md {
        file.push(("Size", format!("{} ({} bytes)", crate::util::format_size(md.len()), md.len())));
    }
    file.push(("Type", crate::fs::formats::type_label(path)));
    let time = |t: std::io::Result<std::time::SystemTime>| {
        t.ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .and_then(|d| glib::DateTime::from_unix_local(d.as_secs() as i64).ok())
            .and_then(|d| d.format("%Y-%m-%d %H:%M:%S").ok())
            .map(|s| s.to_string())
    };
    if let Some(md) = &md {
        if let Some(t) = time(md.modified()) {
            file.push(("Modified", t));
        }
        if let Some(t) = time(md.created()) {
            file.push(("Created", t));
        }
    }
    let mut image = Vec::new();
    if let Some((w, h)) = crate::decode::dimensions(path) {
        image.push(("Dimensions", format!("{w} × {h} ({:.1} MP)", w as f64 * h as f64 / 1e6)));
    }
    if let Some(c) = crate::decode::color_description(path) {
        image.push(("Colour / Bit Depth", c));
    }
    let rating = crate::fs::xattrs::read_rating(path);
    if rating > 0 {
        image.push(("Rating", "★".repeat(rating as usize)));
    }
    let tags = crate::fs::xattrs::read_tags(path);
    if !tags.is_empty() {
        image.push(("Tags", tags.join(", ")));
    }
    PropertyRows { file, image, exif: crate::decode::exif::summary(path) }
}

/// File properties: name, path, size, dimensions, colour, dates, EXIF.
/// File reads happen on a worker; the dialog appears when they are done.
pub fn show_properties(parent: &gtk::Window, path: &Path) {
    let path = path.to_path_buf();
    let parent = parent.clone();
    glib::spawn_future_local(async move {
        let Ok(rows) = gio::spawn_blocking(move || gather_properties(&path)).await else { return };
        present_properties(&parent, rows);
    });
}

fn present_properties(parent: &gtk::Window, rows: PropertyRows) {
    let dialog = adw::Dialog::new();
    dialog.set_title(&tr("Properties"));
    dialog.set_content_width(460);
    let page = adw::PreferencesPage::new();
    let group = |title: Option<&str>, items: &[(&'static str, String)]| {
        let g = adw::PreferencesGroup::new();
        if let Some(t) = title {
            g.set_title(t);
        }
        for (k, v) in items {
            let row = adw::ActionRow::new();
            row.set_title(&tr(k));
            row.set_subtitle(&glib::markup_escape_text(v));
            row.set_subtitle_selectable(true);
            row.add_css_class("property");
            g.add(&row);
        }
        g
    };
    page.add(&group(None, &rows.file));
    if !rows.image.is_empty() {
        page.add(&group(Some(&tr("Image")), &rows.image));
    }
    if !rows.exif.is_empty() {
        page.add(&group(Some("EXIF"), &rows.exif));
    }
    let tv = adw::ToolbarView::new();
    tv.add_top_bar(&adw::HeaderBar::new());
    tv.set_content(Some(&page));
    dialog.set_child(Some(&tv));
    dialog.present(Some(parent));
}

/// Headless smoke test used by CI (`FLOWLIN_SMOKE_TEST=1 flowlin <dir>`):
/// wait for the scan, report counts, toggle recursive, open every image in
/// the viewer (including corrupt ones) and quit.
pub fn smoke_test(w: Window, app: adw::Application) {
    glib::spawn_future_local(async move {
        let wait_scan = |w: &Window| {
            let w = w.clone();
            async move {
                for _ in 0..600 {
                    glib::timeout_future(std::time::Duration::from_millis(50)).await;
                    if w.active_tab().is_some_and(|t| !t.is_scanning()) {
                        return true;
                    }
                }
                false
            }
        };
        let ok = wait_scan(&w).await;
        let tab = w.active_tab().unwrap();
        println!("SMOKE flat={} scanned={ok}", tab.model().total());
        tab.set_recursive(true);
        let ok = wait_scan(&w).await;
        println!("SMOKE recursive={} scanned={ok}", tab.model().total());
        let n = tab.model().n_items();
        tab.open_viewer_at(0);
        for i in 0..n {
            tab.0.viewer.step(if i == 0 { 0 } else { 1 });
            glib::timeout_future(std::time::Duration::from_millis(30)).await;
        }
        glib::timeout_future(std::time::Duration::from_millis(1500)).await;
        tab.close_viewer();
        println!("SMOKE viewed={n}");
        println!("SMOKE ok");
        app.quit();
    });
}
