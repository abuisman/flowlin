//! Main window: header bar, tab strip, sidebar split view and window actions.
//! Actions act on the active tab.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};

use adw::prelude::*;
use gtk::gdk;
use gtk::gio;
use gtk::glib;

use crate::browser::ViewMode;
use crate::fs::ops;
use crate::i18n::tr;
use crate::model::SortKey;
use crate::settings::settings;
use crate::sidebar::{FolderAction, Sidebar};
use crate::tab::{Tab, TabEvent};

pub struct Inner {
    pub window: adw::ApplicationWindow,
    toasts: adw::ToastOverlay,
    toolbar: adw::ToolbarView,
    split: adw::OverlaySplitView,
    tab_view: adw::TabView,
    tab_bar: adw::TabBar,
    sidebar: Sidebar,
    tabs: RefCell<Vec<(adw::TabPage, Tab)>>,
    // header
    star: gtk::Button,
    back: gtk::Button,
    forward: gtk::Button,
    crumbs: gtk::Box,
    crumbs_scroll: gtk::ScrolledWindow,
    path_stack: gtk::Stack,
    path_entry: gtk::Entry,
    count: gtk::Label,
    sort_button: gtk::MenuButton,
    spinner: gtk::Spinner,
    shown_folder: RefCell<PathBuf>,
    /// Sidebar visibility before the viewer took over the window.
    sidebar_before_viewer: Cell<Option<bool>>,
}

#[derive(Clone)]
pub struct Window(pub Rc<Inner>);

fn upgrade(w: &Weak<Inner>) -> Option<Window> {
    w.upgrade().map(Window)
}

impl Window {
    pub fn new(app: &adw::Application) -> Window {
        let s = settings();
        let window = adw::ApplicationWindow::new(app);
        window.set_title(Some(crate::config::APP_NAME));
        window.set_icon_name(Some(crate::config::APP_ID));
        window.set_default_size(s.int("window-width"), s.int("window-height"));
        if s.boolean("window-maximized") {
            window.maximize();
        }
        window.set_size_request(360, 300);

        // ----- header -----
        let header = adw::HeaderBar::new();
        header.set_title_widget(Some(&gtk::Box::new(gtk::Orientation::Horizontal, 0)));

        let sidebar_toggle = gtk::ToggleButton::new();
        sidebar_toggle.set_icon_name("sidebar-show-symbolic");
        sidebar_toggle.set_tooltip_text(Some(&tr("Toggle Sidebar (F9)")));
        header.pack_start(&sidebar_toggle);

        let star = gtk::Button::from_icon_name("non-starred-symbolic");
        star.set_action_name(Some("win.favourite"));
        star.set_tooltip_text(Some(&tr("Add to Favourites (Ctrl+D)")));
        header.pack_start(&star);

        let back = gtk::Button::from_icon_name("go-previous-symbolic");
        back.set_action_name(Some("win.back"));
        back.set_tooltip_text(Some(&tr("Back (Alt+Left)")));
        let forward = gtk::Button::from_icon_name("go-next-symbolic");
        forward.set_action_name(Some("win.forward"));
        forward.set_tooltip_text(Some(&tr("Forward (Alt+Right)")));
        header.pack_start(&back);
        header.pack_start(&forward);

        let crumbs = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        crumbs.add_css_class("breadcrumbs");
        let crumbs_scroll = gtk::ScrolledWindow::new();
        crumbs_scroll.set_policy(gtk::PolicyType::External, gtk::PolicyType::Never);
        crumbs_scroll.set_propagate_natural_width(true);
        crumbs_scroll.set_child(Some(&crumbs));
        // Keep the deepest segment visible whenever the space shrinks.
        crumbs_scroll.hadjustment().connect_changed(|adj| adj.set_value(adj.upper() - adj.page_size()));
        let count = gtk::Label::new(None);
        count.add_css_class("dim-label");
        count.add_css_class("numeric");
        let crumb_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        crumb_row.append(&crumbs_scroll);
        crumb_row.append(&count);
        let path_entry = gtk::Entry::new();
        path_entry.set_hexpand(true);
        path_entry.set_input_purpose(gtk::InputPurpose::Url);
        let path_stack = gtk::Stack::new();
        path_stack.add_named(&crumb_row, Some("crumbs"));
        path_stack.add_named(&path_entry, Some("entry"));
        path_stack.set_hhomogeneous(false);
        path_stack.set_hexpand(true);
        path_stack.set_halign(gtk::Align::Fill);
        crumb_row.set_halign(gtk::Align::Start);
        header.pack_start(&path_stack);

        // Right side, packed right-to-left.
        let fm = gtk::Button::from_icon_name("system-file-manager-symbolic");
        fm.set_action_name(Some("win.open-fm"));
        fm.set_tooltip_text(Some(&tr("Open in File Manager")));
        header.pack_end(&fm);

        let overflow = gtk::MenuButton::new();
        overflow.set_icon_name("open-menu-symbolic");
        overflow.set_tooltip_text(Some(&tr("Main Menu")));
        overflow.set_menu_model(Some(&main_menu()));
        overflow.set_primary(true);
        header.pack_end(&overflow);

        let sort_button = gtk::MenuButton::new();
        sort_button.set_menu_model(Some(&sort_menu()));
        sort_button.set_tooltip_text(Some(&tr("Sort")));
        sort_button.set_always_show_arrow(true);
        header.pack_end(&sort_button);

        let viewer_btn = gtk::Button::from_icon_name("image-x-generic-symbolic");
        viewer_btn.set_action_name(Some("win.open-viewer"));
        viewer_btn.set_tooltip_text(Some(&tr("Open Viewer (Enter)")));
        header.pack_end(&viewer_btn);

        let modes = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        modes.add_css_class("linked");
        for (mode, icon, tip) in [
            ("grid", "view-grid-symbolic", tr("Grid (Ctrl+1)")),
            ("waterfall", "view-dual-symbolic", tr("Waterfall (Ctrl+2)")),
            ("list", "view-list-symbolic", tr("List (Ctrl+3)")),
        ] {
            let b = gtk::ToggleButton::new();
            b.set_icon_name(icon);
            b.set_tooltip_text(Some(&tip));
            b.set_action_name(Some("win.view-mode"));
            b.set_action_target_value(Some(&mode.to_variant()));
            modes.append(&b);
        }
        header.pack_end(&modes);

        let recursive_button = gtk::ToggleButton::new();
        recursive_button.set_icon_name("folder-saved-search-symbolic");
        recursive_button.set_tooltip_text(Some(&tr("Include Subfolders (R)")));
        recursive_button.set_action_name(Some("win.recursive"));
        header.pack_end(&recursive_button);

        let spinner = gtk::Spinner::new();
        spinner.set_tooltip_text(Some(&tr("Scanning…")));
        header.pack_end(&spinner);

        // ----- tabs & split view -----
        let tab_view = adw::TabView::new();
        let tab_bar = adw::TabBar::new();
        tab_bar.set_view(Some(&tab_view));
        tab_bar.set_autohide(false);
        let new_tab = gtk::Button::from_icon_name("tab-new-symbolic");
        new_tab.set_action_name(Some("win.new-tab"));
        new_tab.set_tooltip_text(Some(&tr("New Tab (Ctrl+T)")));
        new_tab.add_css_class("flat");
        tab_bar.set_end_action_widget(Some(&new_tab));
        tab_bar.setup_extra_drop_target(gdk::DragAction::COPY | gdk::DragAction::MOVE, &[gdk::FileList::static_type()]);

        let sidebar = Sidebar::new();
        let split = adw::OverlaySplitView::new();
        split.set_sidebar(Some(sidebar.widget()));
        split.set_content(Some(&tab_view));
        split.set_min_sidebar_width(180.0);
        split.set_max_sidebar_width(s.int("sidebar-width") as f64);
        split.set_sidebar_width_fraction(0.3);
        split.bind_property("show-sidebar", &sidebar_toggle, "active").bidirectional().sync_create().build();
        // Persist only user-initiated sidebar changes, not the viewer hiding it.
        split.set_show_sidebar(s.boolean("sidebar-visible"));

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.add_top_bar(&tab_bar);
        toolbar.set_content(Some(&split));
        toolbar.set_top_bar_style(adw::ToolbarStyle::Raised);
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&toolbar));
        window.set_content(Some(&toasts));

        // Collapse the sidebar into an overlay on narrow windows.
        let bp = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
            adw::BreakpointConditionLengthType::MaxWidth,
            640.0,
            adw::LengthUnit::Sp,
        ));
        bp.add_setter(&split, "collapsed", Some(&true.to_value()));
        window.add_breakpoint(bp);

        let w = Window(Rc::new(Inner {
            window,
            toasts,
            toolbar,
            split,
            tab_view,
            tab_bar,
            sidebar,
            tabs: Default::default(),
            star,
            back,
            forward,
            crumbs,
            crumbs_scroll,
            path_stack,
            path_entry,
            count,
            sort_button,
            spinner,
            shown_folder: Default::default(),
            sidebar_before_viewer: Cell::new(None),
        }));
        w.setup_actions();
        w.setup_signals();
        w
    }

    fn downgrade(&self) -> Weak<Inner> {
        Rc::downgrade(&self.0)
    }

    pub fn window(&self) -> &adw::ApplicationWindow {
        &self.0.window
    }

    pub fn present(&self) {
        self.0.window.present();
        let w = self.downgrade();
        glib::idle_add_local_once(move || {
            let Some(win) = upgrade(&w) else { return };
            if crate::tab::text_entry_has_focus(&win.0.window) {
                return;
            }
            if let Some(t) = win.active_tab() {
                t.focus_view();
            }
        });
    }

    pub fn toast(&self, t: adw::Toast) {
        self.0.toasts.add_toast(t);
    }

    fn toast_text(&self, s: &str) {
        let t = adw::Toast::new(s);
        t.set_timeout(4);
        self.toast(t);
    }

    // ----- tabs -----

    pub fn active_tab(&self) -> Option<Tab> {
        let page = self.0.tab_view.selected_page()?;
        self.tab_for(&page)
    }

    fn tab_for(&self, page: &adw::TabPage) -> Option<Tab> {
        self.0.tabs.borrow().iter().find(|(p, _)| p == page).map(|(_, t)| t.clone())
    }

    fn page_for(&self, tab: &Tab) -> Option<adw::TabPage> {
        self.0.tabs.borrow().iter().find(|(_, t)| t == tab).map(|(p, _)| p.clone())
    }

    /// Open a folder in a new tab and select it.
    pub fn open_tab(&self, folder: &Path) -> Tab {
        let tab = Tab::new(&crate::tab::normalize(folder));
        let page = self.0.tab_view.append(tab.widget());
        page.set_icon(Some(&gio::ThemedIcon::new("folder-symbolic")));
        self.0.tabs.borrow_mut().push((page.clone(), tab.clone()));
        let w = self.downgrade();
        tab.connect_event(move |tab, e| {
            if let Some(win) = upgrade(&w) {
                win.tab_event(tab, e);
            }
        });
        self.update_page_title(&tab);
        self.0.tab_view.set_selected_page(&page);
        // The first page is auto-selected before it is registered above.
        if self.active_tab().as_ref() == Some(&tab) {
            *self.0.shown_folder.borrow_mut() = PathBuf::new();
            self.update_header();
            self.0.sidebar.reveal(&tab.folder());
        }
        tab
    }

    /// Open a file or folder from the command line / file manager.
    pub fn open_path(&self, path: &Path) {
        if path.is_dir() {
            self.open_tab(path);
        } else if let Some(parent) = path.parent() {
            let tab = self.open_tab(parent);
            tab.open_file(path, true);
        }
    }

    fn update_page_title(&self, tab: &Tab) {
        if let Some(page) = self.page_for(tab) {
            page.set_title(&tab.title());
            page.set_tooltip(&glib::markup_escape_text(&tab.folder().to_string_lossy()));
            page.set_loading(tab.is_scanning());
        }
    }

    fn tab_event(&self, tab: &Tab, e: TabEvent) {
        let active = self.active_tab().as_ref() == Some(tab);
        match e {
            TabEvent::Changed => {
                self.update_page_title(tab);
                if active {
                    self.update_header();
                }
            }
            TabEvent::Navigated => {
                self.update_page_title(tab);
                if active {
                    self.update_header();
                    self.0.sidebar.reveal(&tab.folder());
                }
            }
            TabEvent::ViewerOpened => {
                if active {
                    self.viewer_mode(true);
                }
            }
            TabEvent::ViewerClosed => {
                if active {
                    self.viewer_mode(false);
                }
            }
            TabEvent::Toast(t) => self.toast(t),
            TabEvent::FoldersChanged(dir) => self.0.sidebar.refresh(&dir),
            TabEvent::OpenInNewTab(p) => {
                self.open_tab(&p);
            }
            TabEvent::CloseTab => {
                if let Some(page) = self.page_for(tab) {
                    self.0.tab_view.close_page(&page);
                }
            }
        }
    }

    /// The viewer covers the whole window: hide sidebar and bars.
    fn viewer_mode(&self, on: bool) {
        let i = &self.0;
        if on {
            if i.sidebar_before_viewer.get().is_none() {
                i.sidebar_before_viewer.set(Some(i.split.shows_sidebar()));
            }
            i.split.set_show_sidebar(false);
            i.toolbar.set_reveal_top_bars(false);
        } else {
            if let Some(v) = i.sidebar_before_viewer.take() {
                i.split.set_show_sidebar(v);
            }
            i.toolbar.set_reveal_top_bars(true);
        }
    }

    // ----- header -----

    fn update_header(&self) {
        let i = &self.0;
        let Some(tab) = self.active_tab() else { return };
        let folder = tab.folder();
        if *i.shown_folder.borrow() != folder {
            self.rebuild_crumbs(&folder);
            *i.shown_folder.borrow_mut() = folder.clone();
            i.window.set_title(Some(&format!("{} — {}", tab.title(), crate::config::APP_NAME)));
        }
        i.count.set_label(&tab.count_text());
        i.back.set_sensitive(tab.can_go_back());
        i.forward.set_sensitive(tab.can_go_forward());
        let fav = crate::settings::is_favourite(&folder);
        i.star.set_icon_name(if fav { "starred-symbolic" } else { "non-starred-symbolic" });
        i.star.set_tooltip_text(Some(&if fav {
            tr("Remove from Favourites (Ctrl+D)")
        } else {
            tr("Add to Favourites (Ctrl+D)")
        }));
        let scanning = tab.is_scanning();
        i.spinner.set_visible(scanning);
        i.spinner.set_spinning(scanning);
        let st = tab.model().sort_state();
        let arrow = if st.descending { "↓" } else { "↑" };
        i.sort_button.set_label(&format!("{} {arrow}", st.key.short_label()));
        self.set_action_state("recursive", &tab.is_recursive().to_variant());
        self.set_action_state("view-mode", &tab.view_mode().id().to_variant());
        if tab.viewer_open() != i.sidebar_before_viewer.get().is_some() {
            self.viewer_mode(tab.viewer_open());
        }
    }

    fn set_action_state(&self, name: &str, v: &glib::Variant) {
        if let Some(a) = self.0.window.lookup_action(name).and_downcast::<gio::SimpleAction>() {
            if a.state().as_ref() != Some(v) {
                a.set_state(v);
            }
        }
    }

    fn rebuild_crumbs(&self, folder: &Path) {
        let i = &self.0;
        while let Some(c) = i.crumbs.first_child() {
            i.crumbs.remove(&c);
        }
        let crumbs = crate::util::breadcrumbs(folder, &glib::home_dir(), &tr("Home"), &tr("Computer"));
        let n = crumbs.len();
        for (k, c) in crumbs.into_iter().enumerate() {
            if k > 0 {
                let sep = gtk::Label::new(Some("›"));
                sep.add_css_class("dim-label");
                i.crumbs.append(&sep);
            }
            if k + 1 == n {
                // Last segment: a menu of sibling folders.
                let mb = gtk::MenuButton::new();
                mb.set_label(&c.label);
                mb.add_css_class("flat");
                mb.add_css_class("crumb-current");
                mb.set_tooltip_text(Some(&tr("Sibling Folders")));
                // Sibling folders are listed on a worker, never on the UI thread.
                let menu = gio::Menu::new();
                mb.set_menu_model(Some(&menu));
                if let Some(parent) = c.path.parent().map(Path::to_path_buf) {
                    let show_hidden = settings().boolean("show-hidden");
                    glib::spawn_future_local(async move {
                        let sibs = gio::spawn_blocking(move || crate::fs::navigate::subdirs(&parent, show_hidden))
                            .await
                            .unwrap_or_default();
                        for sib in sibs.into_iter().take(300) {
                            let item = gio::MenuItem::new(Some(&crate::util::file_name(&sib)), None);
                            item.set_action_and_target_value(Some("win.goto"), Some(&path_uri(&sib).to_variant()));
                            menu.append_item(&item);
                        }
                    });
                }
                i.crumbs.append(&mb);
            } else {
                let b = gtk::Button::with_label(&c.label);
                b.add_css_class("flat");
                b.set_action_name(Some("win.goto"));
                b.set_action_target_value(Some(&path_uri(&c.path).to_variant()));
                i.crumbs.append(&b);
            }
        }
        // Keep the deepest segment visible.
        let sc = i.crumbs_scroll.clone();
        glib::idle_add_local_once(move || {
            let adj = sc.hadjustment();
            adj.set_value(adj.upper());
        });
    }

    fn show_location_entry(&self) {
        let i = &self.0;
        let Some(tab) = self.active_tab() else { return };
        i.path_entry.set_text(&tab.folder().to_string_lossy());
        i.path_stack.set_visible_child_name("entry");
        i.path_entry.grab_focus();
        i.path_entry.select_region(0, -1);
    }

    fn hide_location_entry(&self) {
        self.0.path_stack.set_visible_child_name("crumbs");
        if let Some(t) = self.active_tab() {
            t.focus_view();
        }
    }

    // ----- setup -----

    fn setup_signals(&self) {
        let i = &self.0;
        let w = self.downgrade();
        i.split.connect_show_sidebar_notify(move |split| {
            if let Some(win) = upgrade(&w) {
                if win.0.sidebar_before_viewer.get().is_none() {
                    let _ = settings().set_boolean("sidebar-visible", split.shows_sidebar());
                }
            }
        });
        let w = self.downgrade();
        i.tab_view.connect_selected_page_notify(move |_| {
            if let Some(win) = upgrade(&w) {
                *win.0.shown_folder.borrow_mut() = PathBuf::new();
                win.update_header();
                if let Some(t) = win.active_tab() {
                    win.0.sidebar.reveal(&t.folder());
                    win.viewer_mode(t.viewer_open());
                    // Keep keyboard focus in the visible tab (e.g. after closing one).
                    let w = win.downgrade();
                    glib::idle_add_local_once(move || {
                        let Some(win) = upgrade(&w) else { return };
                        if !crate::tab::text_entry_has_focus(&win.0.window) {
                            if let Some(t) = win.active_tab() {
                                t.focus_view();
                            }
                        }
                    });
                }
            }
        });
        let w = self.downgrade();
        i.tab_view.connect_close_page(move |view, page| {
            if let Some(win) = upgrade(&w) {
                let tab = win.tab_for(page);
                win.0.tabs.borrow_mut().retain(|(p, _)| p != page);
                if let Some(t) = tab {
                    t.shutdown();
                }
                view.close_page_finish(page, true);
                if view.n_pages() == 0 {
                    win.0.window.close();
                }
            }
            glib::Propagation::Stop
        });
        let w = self.downgrade();
        i.tab_bar.connect_extra_drag_drop(move |bar, page, value| {
            let Some(win) = upgrade(&w) else { return false };
            let Some(tab) = win.tab_for(page) else { return false };
            let Ok(list) = value.get::<gdk::FileList>() else { return false };
            let paths: Vec<PathBuf> = list.files().iter().filter_map(|f| f.path()).collect();
            if paths.len() == 1 && paths[0].is_dir() {
                tab.navigate(&paths[0], true, true);
            } else {
                let mode = if bar.extra_drag_preferred_action() == gdk::DragAction::COPY {
                    ops::Transfer::Copy
                } else {
                    ops::Transfer::Move
                };
                let dest = tab.folder();
                glib::spawn_future_local(async move {
                    tab.transfer_into(paths, &dest, mode).await;
                });
            }
            true
        });

        let w = self.downgrade();
        i.sidebar.connect_open(move |path, new_tab| {
            let Some(win) = upgrade(&w) else { return };
            if new_tab {
                win.open_tab(&path);
            } else if let Some(t) = win.active_tab() {
                t.navigate(&path, true, false);
            }
        });
        let w = self.downgrade();
        i.sidebar.connect_action(move |action, path| {
            if let Some(win) = upgrade(&w) {
                win.folder_action(action, path);
            }
        });
        // Drop files onto a sidebar folder: handled via a drop target on the list.
        self.setup_sidebar_drop();

        let w = self.downgrade();
        i.path_entry.connect_activate(move |e| {
            let Some(win) = upgrade(&w) else { return };
            let text = e.text().to_string();
            let path = if let Some(rest) = text.strip_prefix('~') {
                PathBuf::from(format!("{}{}", glib::home_dir().display(), rest))
            } else {
                PathBuf::from(text)
            };
            win.hide_location_entry();
            if let Some(t) = win.active_tab() {
                if path.is_file() {
                    t.open_file(&path, true);
                } else {
                    t.navigate(&path, true, true);
                }
            }
        });
        let keys = gtk::EventControllerKey::new();
        let w = self.downgrade();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                if let Some(win) = upgrade(&w) {
                    win.hide_location_entry();
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        i.path_entry.add_controller(keys);
        let focus = gtk::EventControllerFocus::new();
        let w = self.downgrade();
        focus.connect_leave(move |_| {
            if let Some(win) = upgrade(&w) {
                win.0.path_stack.set_visible_child_name("crumbs");
            }
        });
        i.path_entry.add_controller(focus);

        // React to preference changes.
        let s = settings();
        let w = self.downgrade();
        s.connect_changed(None, move |s, key| {
            let Some(win) = upgrade(&w) else { return };
            let tabs: Vec<Tab> = win.0.tabs.borrow().iter().map(|(_, t)| t.clone()).collect();
            match key {
                "thumbnail-size" => tabs.iter().for_each(|t| t.set_thumb_size(crate::settings::thumb_size())),
                "show-names" => tabs.iter().for_each(|t| t.set_show_names(s.boolean("show-names"))),
                "show-folder-labels" => tabs.iter().for_each(|t| t.set_show_folder_labels(s.boolean(key))),
                "show-hidden" | "same-device" | "recursive-cap" | "show-videos" => tabs.iter().for_each(|t| t.reload()),
                "sort-key" | "sort-descending" => {
                    let k = SortKey::from_id(&s.string("sort-key"));
                    let d = s.boolean("sort-descending");
                    tabs.iter().for_each(|t| t.set_sort(k, d));
                    win.set_action_state("sort-key", &k.id().to_variant());
                    win.set_action_state("sort-descending", &d.to_variant());
                }
                "view-mode" => {
                    let m = ViewMode::from_id(&s.string("view-mode"));
                    tabs.iter().for_each(|t| t.set_view_mode(m));
                }
                "thumbnail-memory" => crate::thumbs::ThumbService::get().set_memory_cap(s.uint(key)),
                "favourites" => {}
                _ => return,
            }
            win.update_header();
        });

        let w = self.downgrade();
        i.window.connect_close_request(move |win| {
            let s = settings();
            if !win.is_maximized() && !win.is_fullscreen() {
                let (wd, ht) = win.default_size();
                let _ = s.set_int("window-width", wd);
                let _ = s.set_int("window-height", ht);
            }
            let _ = s.set_boolean("window-maximized", win.is_maximized());
            if let Some(w) = upgrade(&w) {
                for (_, t) in w.0.tabs.borrow().iter() {
                    t.shutdown();
                }
            }
            glib::Propagation::Proceed
        });
    }

    fn setup_sidebar_drop(&self) {
        let target = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY | gdk::DragAction::MOVE);
        target.set_preload(false);
        let w = self.downgrade();
        target.connect_drop(move |t, value, x, y| {
            let Some(win) = upgrade(&w) else { return false };
            let Ok(list) = value.get::<gdk::FileList>() else { return false };
            let paths: Vec<PathBuf> = list.files().iter().filter_map(|f| f.path()).collect();
            let widget = win.0.sidebar.widget().clone();
            let Some(dest) = sidebar_folder_at(&widget, x, y) else { return false };
            let copy = t.current_event_state().contains(gdk::ModifierType::CONTROL_MASK);
            let mode = if copy { ops::Transfer::Copy } else { ops::Transfer::Move };
            let Some(tab) = win.active_tab() else { return false };
            glib::spawn_future_local(async move {
                tab.transfer_into(paths, &dest, mode).await;
            });
            true
        });
        self.0.sidebar.widget().add_controller(target);
    }

    fn folder_action(&self, action: FolderAction, path: PathBuf) {
        let win: gtk::Window = self.0.window.clone().upcast();
        match action {
            FolderAction::Open => {
                if let Some(t) = self.active_tab() {
                    t.navigate(&path, true, true);
                }
            }
            FolderAction::View => {
                if let Some(t) = self.active_tab() {
                    t.open_first_image(&path);
                }
            }
            FolderAction::OpenRecursive => {
                if let Some(t) = self.active_tab() {
                    t.navigate_recursive(&path);
                }
            }
            FolderAction::OpenInNewTab => {
                self.open_tab(&path);
            }
            FolderAction::OpenInFileManager => ops::open_folder_in_file_manager(&path, Some(&win)),
            FolderAction::ToggleFavourite => {
                crate::settings::toggle_favourite(&path);
                self.update_header();
            }
            FolderAction::Rename => {
                let w = self.downgrade();
                crate::tab::rename_dialog(&win, &path, move |old, new| {
                    if let Some(win) = upgrade(&w) {
                        if let Some(p) = old.parent() {
                            win.0.sidebar.refresh(p);
                        }
                        for (_, t) in win.0.tabs.borrow().iter() {
                            if t.folder().starts_with(old) {
                                let rest =
                                    t.folder().strip_prefix(old).map(|r| new.join(r)).unwrap_or(new.to_path_buf());
                                t.navigate(&rest, false, false);
                            }
                        }
                    }
                });
            }
            FolderAction::NewFolder => {
                let w = self.downgrade();
                let parent = path.clone();
                crate::tab::name_dialog(&win, &tr("New Folder"), &tr("Create"), &tr("New Folder"), None, move |name| {
                    let parent = parent.clone();
                    let w = w.clone();
                    glib::spawn_future_local(async move {
                        let r = ops::new_folder(&parent, &name).await;
                        if let Some(win) = upgrade(&w) {
                            match r {
                                Ok(_) => win.0.sidebar.refresh(&parent),
                                Err(e) => win.toast_text(e.message()),
                            }
                        }
                    });
                });
            }
            FolderAction::Trash => {
                let w = self.downgrade();
                glib::spawn_future_local(async move {
                    let (done, errs) = ops::trash(std::slice::from_ref(&path)).await;
                    let Some(win) = upgrade(&w) else { return };
                    if let Some(e) = errs.first() {
                        win.toast_text(e.message());
                        return;
                    }
                    if let Some(p) = path.parent() {
                        win.0.sidebar.refresh(p);
                        for (_, t) in win.0.tabs.borrow().iter() {
                            if t.folder().starts_with(&path) {
                                t.navigate(p, true, false);
                            }
                        }
                    }
                    let toast =
                        adw::Toast::new(&format!("“{}” {}", crate::util::file_name(&path), tr("moved to trash")));
                    toast.set_button_label(Some(&tr("Undo")));
                    toast.set_timeout(10);
                    let w2 = w.clone();
                    toast.connect_button_clicked(move |_| {
                        let done = done.clone();
                        let w3 = w2.clone();
                        glib::spawn_future_local(async move {
                            ops::restore_from_trash(&done).await;
                            if let (Some(win), Some(p)) = (upgrade(&w3), done.first().and_then(|d| d.parent())) {
                                win.0.sidebar.refresh(p);
                            }
                        });
                    });
                    win.toast(toast);
                });
            }
        }
    }

    fn setup_actions(&self) {
        let win = &self.0.window;
        let add = |name: &str, f: Box<dyn Fn(&Window)>| {
            let a = gio::SimpleAction::new(name, None);
            let w = self.downgrade();
            a.connect_activate(move |_, _| {
                if let Some(win) = upgrade(&w) {
                    f(&win);
                }
            });
            win.add_action(&a);
        };
        let tab_action = |name: &str, f: fn(&Tab, &Window)| {
            add(
                name,
                Box::new(move |w: &Window| {
                    if let Some(t) = w.active_tab() {
                        f(&t, w);
                    }
                }),
            );
        };

        add(
            "new-tab",
            Box::new(|w| {
                let folder = w.active_tab().map(|t| t.folder()).unwrap_or_else(glib::home_dir);
                w.open_tab(&folder);
            }),
        );
        add(
            "close-tab",
            Box::new(|w| {
                if let Some(p) = w.0.tab_view.selected_page() {
                    w.0.tab_view.close_page(&p);
                }
            }),
        );
        tab_action("back", |t, _| {
            if t.viewer_open() {
                t.close_viewer();
            }
            t.go_back()
        });
        tab_action("forward", |t, _| t.go_forward());
        tab_action("up", |t, _| t.go_up());
        tab_action("favourite", |t, w| {
            let now = crate::settings::toggle_favourite(&t.folder());
            w.toast_text(&if now { tr("Added to favourites") } else { tr("Removed from favourites") });
            w.update_header();
        });
        add("location", Box::new(|w| w.show_location_entry()));
        tab_action("open-viewer", |t, _| t.open_viewer());
        tab_action("open-fm", |t, w| ops::open_folder_in_file_manager(&t.folder(), Some(w.0.window.upcast_ref())));
        tab_action("show-in-fm", |t, w| {
            if let Some(p) = t.selected_paths().first() {
                ops::show_in_file_manager(p, Some(w.0.window.upcast_ref()));
            }
        });
        tab_action("open-with", |t, w| {
            if let Some(p) = t.selected_paths().first() {
                ops::open_with(p, Some(w.0.window.upcast_ref()));
            }
        });
        tab_action("refresh", |t, _| t.reload());
        tab_action("filter", |t, _| t.toggle_filter());
        tab_action("copy", |t, _| t.copy_to_clipboard(false));
        tab_action("cut", |t, _| t.copy_to_clipboard(true));
        tab_action("paste", |t, _| t.paste());
        tab_action("copy-path", |t, _| t.copy_path());
        tab_action("trash", |t, _| t.trash_selected());
        tab_action("delete-permanently", |t, w| t.delete_selected_permanently(w.0.window.upcast_ref()));
        tab_action("rename", |t, w| t.rename_selected(w.0.window.upcast_ref()));
        tab_action("new-folder", |t, w| t.new_folder(w.0.window.upcast_ref()));
        tab_action("properties", |t, w| {
            if let Some(p) = t.selected_paths().first() {
                crate::app::show_properties(w.0.window.upcast_ref(), p);
            }
        });
        tab_action("jump-prev", |t, _| t.jump_folder(false));
        tab_action("jump-next", |t, _| t.jump_folder(true));
        add(
            "zoom-in",
            Box::new(|_| {
                let s = settings();
                let _ = s.set_int("thumbnail-size", (s.int("thumbnail-size") + 1).min(4));
            }),
        );
        add(
            "zoom-out",
            Box::new(|_| {
                let s = settings();
                let _ = s.set_int("thumbnail-size", (s.int("thumbnail-size") - 1).max(0));
            }),
        );
        add(
            "fullscreen",
            Box::new(|w| {
                let win = &w.0.window;
                if win.is_fullscreen() {
                    win.unfullscreen();
                } else {
                    win.fullscreen();
                }
            }),
        );
        add("toggle-sidebar", Box::new(|w| w.0.split.set_show_sidebar(!w.0.split.shows_sidebar())));
        add(
            "focus-sidebar",
            Box::new(|w| {
                w.0.split.set_show_sidebar(true);
                w.0.sidebar.focus();
            }),
        );

        // goto(path)
        let goto = gio::SimpleAction::new("goto", Some(glib::VariantTy::STRING));
        let w = self.downgrade();
        goto.connect_activate(move |_, v| {
            let (Some(win), Some(uri)) = (upgrade(&w), v.and_then(|v| v.get::<String>())) else { return };
            // Targets are file:// URIs so non-UTF-8 names survive.
            let Some(p) = gio::File::for_uri(&uri).path() else { return };
            if let Some(t) = win.active_tab() {
                t.navigate(&p, true, true);
            }
        });
        win.add_action(&goto);

        // Stateful actions.
        let s = settings();
        let view_mode = gio::SimpleAction::new_stateful(
            "view-mode",
            Some(glib::VariantTy::STRING),
            &s.string("view-mode").to_variant(),
        );
        view_mode.connect_activate(|a, v| {
            if let Some(v) = v {
                a.set_state(v);
                let _ = settings().set_string("view-mode", v.str().unwrap_or("grid"));
            }
        });
        win.add_action(&view_mode);

        let sort_key = gio::SimpleAction::new_stateful(
            "sort-key",
            Some(glib::VariantTy::STRING),
            &s.string("sort-key").to_variant(),
        );
        sort_key.connect_activate(|a, v| {
            if let Some(v) = v {
                a.set_state(v);
                let _ = settings().set_string("sort-key", v.str().unwrap_or("name"));
            }
        });
        win.add_action(&sort_key);
        win.add_action(&s.create_action("sort-descending"));
        win.add_action(&s.create_action("show-hidden"));
        win.add_action(&s.create_action("show-videos"));
        win.add_action(&s.create_action("show-names"));
        win.add_action(&s.create_action("show-folder-labels"));

        let recursive = gio::SimpleAction::new_stateful("recursive", None, &false.to_variant());
        let w = self.downgrade();
        recursive.connect_activate(move |a, _| {
            let Some(win) = upgrade(&w) else { return };
            if let Some(t) = win.active_tab() {
                let on = !t.is_recursive();
                t.set_recursive(on);
                a.set_state(&on.to_variant());
                win.update_header();
            }
        });
        win.add_action(&recursive);
    }
}

fn path_uri(p: &Path) -> String {
    gio::File::for_path(p).uri().to_string()
}

/// Folder path under a point in the sidebar list.
fn sidebar_folder_at(widget: &gtk::ScrolledWindow, x: f64, y: f64) -> Option<PathBuf> {
    let mut w = widget.pick(x, y, gtk::PickFlags::DEFAULT);
    while let Some(cur) = w {
        if let Some(row) = cur.downcast_ref::<crate::sidebar::Row>() {
            return row.folder_path();
        }
        w = cur.parent();
    }
    None
}

fn sort_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    let keys = gio::Menu::new();
    for k in SortKey::ALL {
        let item = gio::MenuItem::new(Some(&k.label()), None);
        item.set_action_and_target_value(Some("win.sort-key"), Some(&k.id().to_variant()));
        keys.append_item(&item);
    }
    menu.append_section(None, &keys);
    let dir = gio::Menu::new();
    dir.append(Some(&tr("Descending")), Some("win.sort-descending"));
    menu.append_section(None, &dir);
    menu
}

fn main_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    let s1 = gio::Menu::new();
    s1.append(Some(&tr("New Tab")), Some("win.new-tab"));
    s1.append(Some(&tr("New Folder…")), Some("win.new-folder"));
    menu.append_section(None, &s1);
    let s2 = gio::Menu::new();
    s2.append(Some(&tr("Include Subfolders")), Some("win.recursive"));
    s2.append(Some(&tr("Show Hidden Files")), Some("win.show-hidden"));
    s2.append(Some(&tr("Show Videos")), Some("win.show-videos"));
    s2.append(Some(&tr("Show File Names")), Some("win.show-names"));
    s2.append(Some(&tr("Show Folder Under Name (Recursive)")), Some("win.show-folder-labels"));
    menu.append_section(None, &s2);
    let size = gio::Menu::new();
    size.append(Some(&tr("Larger Thumbnails")), Some("win.zoom-in"));
    size.append(Some(&tr("Smaller Thumbnails")), Some("win.zoom-out"));
    menu.append_section(Some(&tr("Thumbnail Size")), &size);
    let s3 = gio::Menu::new();
    s3.append(Some(&tr("Preferences")), Some("app.preferences"));
    s3.append(Some(&tr("Keyboard Shortcuts")), Some("app.shortcuts"));
    s3.append(Some(&tr("About Flowlin")), Some("app.about"));
    menu.append_section(None, &s3);
    menu
}
