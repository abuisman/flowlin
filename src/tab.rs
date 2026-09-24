//! One browser tab: its folder, navigation history, recursive flag, model,
//! views, viewer overlay and file monitor. Nothing here is persisted.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use adw::prelude::*;
use gtk::gdk;
use gtk::gio;
use gtk::glib;

use crate::browser::list::ListView;
use crate::browser::waterfall::WaterfallView;
use crate::browser::{grid, ThumbCell, ViewConfig, ViewMode};
use crate::fs::ops::{self, Transfer};
use crate::fs::scan::{self, Cancel, FileEntry, ScanMsg, ScanOptions};
use crate::i18n::{tr, trn};
use crate::model::{BrowserModel, ImageItem, SortKey, ThumbState};
use crate::settings::settings;
use crate::util::format_count;
use crate::viewer::Viewer;

/// Right-button drag gestures (FlowVision).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gesture {
    Up,
    Down,
    Left,
    Right,
    UpRight,
    DownRight,
}

/// Minimum drag distance for a gesture; shorter drags open the menu.
pub const GESTURE_MIN: f64 = 40.0;

/// Classify a drag vector (screen coordinates, y grows downwards).
pub fn classify_gesture(dx: f64, dy: f64) -> Option<Gesture> {
    if dx.hypot(dy) < GESTURE_MIN {
        return None;
    }
    let angle = (-dy).atan2(dx).to_degrees(); // 0 = right, 90 = up
    Some(match angle {
        a if (-22.5..22.5).contains(&a) => Gesture::Right,
        a if (22.5..67.5).contains(&a) => Gesture::UpRight,
        a if (67.5..112.5).contains(&a) => Gesture::Up,
        a if (-67.5..-22.5).contains(&a) => Gesture::DownRight,
        a if (-112.5..-67.5).contains(&a) => Gesture::Down,
        a if !(-157.5..157.5).contains(&a) => Gesture::Left,
        // Up-left / down-left diagonals: treat as the dominant axis.
        _ if dx.abs() > dy.abs() => Gesture::Left,
        _ if dy < 0.0 => Gesture::Up,
        _ => Gesture::Down,
    })
}

/// Notifications from a tab to its window.
pub enum TabEvent {
    /// Folder, counts, flags or scan state changed: refresh the header.
    Changed,
    /// Navigated to a new folder.
    Navigated,
    ViewerOpened,
    ViewerClosed,
    Toast(adw::Toast),
    /// A folder's subfolders changed (sidebar refresh).
    FoldersChanged(PathBuf),
    OpenInNewTab(PathBuf),
    /// The down-right gesture asks to close this tab.
    CloseTab,
}

type EventHandler = Box<dyn Fn(&Tab, TabEvent)>;

pub struct Inner {
    pub root: gtk::Overlay,
    trail: gtk::DrawingArea,
    trail_points: Rc<RefCell<Vec<(f64, f64)>>>,
    stack: gtk::Stack,
    search_bar: gtk::SearchBar,
    search_entry: gtk::SearchEntry,
    empty_page: adw::StatusPage,
    error_page: adw::StatusPage,
    pub model: BrowserModel,
    cfg: Rc<ViewConfig>,
    grid: gtk::GridView,
    list: ListView,
    waterfall: WaterfallView,
    pub viewer: Viewer,
    folder: RefCell<PathBuf>,
    back: RefCell<Vec<PathBuf>>,
    forward: RefCell<Vec<PathBuf>>,
    recursive: Cell<bool>,
    scanning: Cell<bool>,
    /// The scan found files; show the view page even before they are added.
    expect_items: Cell<bool>,
    truncated: Cell<bool>,
    scan_error: RefCell<Option<String>>,
    scan_cancel: RefCell<Option<Cancel>>,
    dims_cancel: RefCell<Option<Cancel>>,
    ratings_cancel: RefCell<Option<Cancel>>,
    monitor: RefCell<Option<gio::FileMonitor>>,
    mode: Cell<ViewMode>,
    /// Select (and optionally open) this file once it shows up in the model.
    pending_select: RefCell<Option<(PathBuf, bool)>>,
    focus_after_load: Cell<bool>,
    /// Open the viewer on the first image when the current scan finishes.
    open_first_after_load: Cell<bool>,
    typeahead: RefCell<String>,
    typeahead_at: Cell<Option<Instant>>,
    xattr_warned: Cell<bool>,
    on_event: RefCell<Option<EventHandler>>,
    settings_handlers: RefCell<Vec<glib::SignalHandlerId>>,
}

#[derive(Clone)]
pub struct Tab(pub Rc<Inner>);

impl PartialEq for Tab {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

fn upgrade(w: &Weak<Inner>) -> Option<Tab> {
    w.upgrade().map(Tab)
}

fn scrolled(child: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    let s = gtk::ScrolledWindow::new();
    s.set_hscrollbar_policy(gtk::PolicyType::Never);
    s.set_child(Some(child));
    s.set_vexpand(true);
    s
}

impl Tab {
    pub fn new(folder: &Path) -> Tab {
        let model = BrowserModel::new();
        let s = settings();
        model.set_sort(SortKey::from_id(&s.string("sort-key")), s.boolean("sort-descending"));
        let cfg = Rc::new(ViewConfig::default());
        cfg.size.set(crate::settings::thumb_size());
        cfg.show_names.set(s.boolean("show-names"));

        let grid = grid::create(&model, cfg.clone());
        let waterfall = WaterfallView::new(&model.selection, cfg.clone());
        let list = ListView::new(&model, move |key, desc| {
            let s = settings();
            let _ = s.set_string("sort-key", key.id());
            let _ = s.set_boolean("sort-descending", desc);
        });

        let stack = gtk::Stack::new();
        stack.set_transition_type(gtk::StackTransitionType::None);
        let grid_scroll = scrolled(&grid);
        {
            let (g, c) = (grid.clone(), cfg.clone());
            grid_scroll.hadjustment().connect_changed(move |adj| grid::fit_columns(&g, adj.page_size(), &c));
        }
        stack.add_named(&grid_scroll, Some("grid"));
        stack.add_named(&scrolled(&waterfall), Some("waterfall"));
        stack.add_named(&scrolled(&list.view), Some("list"));

        let loading = adw::StatusPage::new();
        let spinner = gtk::Spinner::new();
        spinner.set_spinning(true);
        spinner.set_size_request(32, 32);
        loading.set_child(Some(&spinner));
        loading.set_title(&tr("Loading…"));
        stack.add_named(&loading, Some("loading"));

        let empty_page = adw::StatusPage::new();
        empty_page.set_icon_name(Some("image-x-generic-symbolic"));
        empty_page.set_title(&tr("No Images in This Folder"));
        let rec_btn = gtk::Button::with_label(&tr("Include Subfolders"));
        rec_btn.add_css_class("pill");
        rec_btn.add_css_class("suggested-action");
        rec_btn.set_halign(gtk::Align::Center);
        rec_btn.set_action_name(Some("win.recursive"));
        empty_page.set_child(Some(&rec_btn));
        stack.add_named(&empty_page, Some("empty"));

        let error_page = adw::StatusPage::new();
        error_page.set_icon_name(Some("dialog-warning-symbolic"));
        error_page.set_title(&tr("Cannot Open Folder"));
        stack.add_named(&error_page, Some("error"));

        let search_entry = gtk::SearchEntry::new();
        search_entry.set_placeholder_text(Some(&tr("Filter by name, or rating:>=3")));
        search_entry.set_hexpand(true);
        let clamp = adw::Clamp::new();
        clamp.set_maximum_size(480);
        clamp.set_child(Some(&search_entry));
        let search_bar = gtk::SearchBar::new();
        search_bar.set_child(Some(&clamp));
        search_bar.connect_entry(&search_entry);
        search_bar.set_show_close_button(true);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&search_bar);
        content.append(&stack);

        let viewer = Viewer::new(&model);
        let root = gtk::Overlay::new();
        root.set_child(Some(&content));
        let trail = gtk::DrawingArea::new();
        trail.set_can_target(false);
        let trail_points: Rc<RefCell<Vec<(f64, f64)>>> = Default::default();
        let tp = trail_points.clone();
        trail.set_draw_func(move |area, cr, _, _| {
            let pts = tp.borrow();
            if pts.len() < 2 {
                return;
            }
            let c = area.color();
            cr.set_source_rgba(c.red() as f64, c.green() as f64, c.blue() as f64, 0.35);
            cr.set_line_width(4.0);
            cr.set_line_cap(gtk::cairo::LineCap::Round);
            cr.set_line_join(gtk::cairo::LineJoin::Round);
            cr.move_to(pts[0].0, pts[0].1);
            for p in pts.iter().skip(1) {
                cr.line_to(p.0, p.1);
            }
            let _ = cr.stroke();
        });
        root.add_overlay(&trail);
        root.add_overlay(viewer.widget());

        let tab = Tab(Rc::new(Inner {
            root,
            trail,
            trail_points,
            stack,
            search_bar,
            search_entry,
            empty_page,
            error_page,
            model,
            cfg,
            grid,
            list,
            waterfall,
            viewer,
            folder: RefCell::new(folder.to_path_buf()),
            back: Default::default(),
            forward: Default::default(),
            recursive: Cell::new(false),
            scanning: Cell::new(false),
            expect_items: Cell::new(false),
            truncated: Cell::new(false),
            scan_error: Default::default(),
            scan_cancel: Default::default(),
            dims_cancel: Default::default(),
            ratings_cancel: Default::default(),
            monitor: Default::default(),
            mode: Cell::new(ViewMode::from_id(&s.string("view-mode"))),
            pending_select: Default::default(),
            focus_after_load: Cell::new(true),
            open_first_after_load: Cell::new(false),
            typeahead: Default::default(),
            typeahead_at: Cell::new(None),
            xattr_warned: Cell::new(false),
            on_event: Default::default(),
            settings_handlers: Default::default(),
        }));
        tab.setup();
        tab.reload();
        tab
    }

    fn downgrade(&self) -> Weak<Inner> {
        Rc::downgrade(&self.0)
    }

    pub fn widget(&self) -> &gtk::Overlay {
        &self.0.root
    }

    pub fn connect_event(&self, f: impl Fn(&Tab, TabEvent) + 'static) {
        *self.0.on_event.borrow_mut() = Some(Box::new(f));
    }

    fn emit(&self, e: TabEvent) {
        if let Some(f) = self.0.on_event.borrow().as_ref() {
            f(self, e);
        }
    }

    pub fn toast(&self, t: adw::Toast) {
        self.emit(TabEvent::Toast(t));
    }

    fn toast_text(&self, s: &str) {
        let t = adw::Toast::new(s);
        t.set_timeout(4);
        self.toast(t);
    }

    // ----- state accessors -----

    pub fn folder(&self) -> PathBuf {
        self.0.folder.borrow().clone()
    }
    pub fn title(&self) -> String {
        let f = self.folder();
        if f == glib::home_dir() {
            return tr("Home");
        }
        if f == Path::new("/") {
            return tr("Computer");
        }
        crate::util::file_name(&f)
    }
    pub fn can_go_back(&self) -> bool {
        !self.0.back.borrow().is_empty()
    }
    pub fn can_go_forward(&self) -> bool {
        !self.0.forward.borrow().is_empty()
    }
    pub fn is_recursive(&self) -> bool {
        self.0.recursive.get()
    }
    pub fn is_scanning(&self) -> bool {
        self.0.scanning.get()
    }
    pub fn viewer_open(&self) -> bool {
        self.0.viewer.is_open()
    }
    pub fn model(&self) -> &BrowserModel {
        &self.0.model
    }

    /// "(107 images)", "(12 of 107 images)", "(4 382 images, recursive)".
    pub fn count_text(&self) -> String {
        let m = &self.0.model;
        let total = m.total();
        let shown = m.n_items();
        let noun = trn("image", "images", total);
        let mut s = if m.is_filtered() {
            format!("{} {} {} {}", format_count(shown as usize), tr("of"), format_count(total as usize), noun)
        } else {
            format!("{} {}", format_count(total as usize), noun)
        };
        if self.is_recursive() {
            s.push_str(&format!(", {}", tr("recursive")));
        }
        format!("({s})")
    }

    // ----- navigation -----

    /// Navigate to `path`. `focus_view` moves keyboard focus to the
    /// thumbnails once loaded (not wanted when the sidebar drove it).
    pub fn navigate(&self, path: &Path, push_history: bool, focus_view: bool) {
        let path = normalize(path);
        if !path.is_dir() {
            self.toast_text(&format!("{}: {}", tr("Not a folder"), path.display()));
            return;
        }
        let cur = self.folder();
        if cur == path {
            return;
        }
        self.go_to(path, cur, push_history, focus_view);
    }

    fn go_to(&self, path: PathBuf, cur: PathBuf, push_history: bool, focus_view: bool) {
        if push_history {
            self.0.back.borrow_mut().push(cur);
            self.0.forward.borrow_mut().clear();
        }
        *self.0.folder.borrow_mut() = path;
        self.0.focus_after_load.set(focus_view);
        self.0.search_entry.set_text("");
        self.0.search_bar.set_search_mode(false);
        self.reload();
        self.emit(TabEvent::Navigated);
    }

    /// Open `file`'s folder, select it and optionally open the viewer on it.
    pub fn open_file(&self, file: &Path, show_viewer: bool) {
        let file = normalize(file);
        *self.0.pending_select.borrow_mut() = Some((file.clone(), show_viewer));
        if let Some(parent) = file.parent() {
            if parent == self.folder() {
                self.apply_pending_select();
            } else {
                self.navigate(parent, true, true);
            }
        }
    }

    pub fn go_back(&self) {
        let Some(p) = self.0.back.borrow_mut().pop() else { return };
        let cur = self.folder();
        self.0.forward.borrow_mut().push(cur.clone());
        self.go_to(p, cur, false, true);
    }

    pub fn go_forward(&self) {
        let Some(p) = self.0.forward.borrow_mut().pop() else { return };
        let cur = self.folder();
        self.0.back.borrow_mut().push(cur.clone());
        self.go_to(p, cur, false, true);
    }

    pub fn go_up(&self) {
        if let Some(parent) = self.folder().parent() {
            self.navigate(parent, true, true);
        }
    }

    /// A / D: previous / next folder that contains images (depth-first).
    pub fn jump_folder(&self, forward: bool) {
        let start = self.folder();
        let origin = start.clone();
        let show_hidden = settings().boolean("show-hidden");
        let (tx, rx) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let _ = tx.send_blocking(crate::fs::navigate::find_image_folder(&start, forward, show_hidden, 20_000));
        });
        let w = self.downgrade();
        glib::spawn_future_local(async move {
            let Ok(res) = rx.recv().await else { return };
            let Some(t) = upgrade(&w) else { return };
            if t.folder() != origin {
                return; // the user moved on while we searched
            }
            match res {
                Some(p) => t.navigate(&p, true, true),
                None => t.toast_text(&if forward {
                    tr("No next folder with images")
                } else {
                    tr("No previous folder with images")
                }),
            }
        });
    }

    /// Next sibling folder with images (up-right gesture).
    pub fn jump_sibling(&self) {
        let start = self.folder();
        let origin = start.clone();
        let show_hidden = settings().boolean("show-hidden");
        let (tx, rx) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let _ = tx.send_blocking(crate::fs::navigate::next_sibling_with_images(&start, show_hidden));
        });
        let w = self.downgrade();
        glib::spawn_future_local(async move {
            let Ok(res) = rx.recv().await else { return };
            let Some(t) = upgrade(&w) else { return };
            if t.folder() != origin {
                return;
            }
            match res {
                Some(p) => t.navigate(&p, true, true),
                None => t.toast_text(&tr("No next sibling folder with images")),
            }
        });
    }

    pub fn set_recursive(&self, on: bool) {
        if self.set_recursive_flag(on) {
            self.reload();
        }
    }

    /// Open `path` and show its first image in the viewer once loaded.
    pub fn open_first_image(&self, path: &Path) {
        let path = normalize(path);
        if path == self.folder() && !self.is_scanning() {
            if self.0.model.n_items() > 0 {
                self.open_viewer_at(0);
            } else {
                self.toast_text(&tr("No images in this folder"));
            }
            return;
        }
        self.navigate(&path, true, true);
        if self.folder() == path {
            self.0.open_first_after_load.set(true);
        }
    }

    /// Open `path` with subfolders included (a single scan).
    pub fn navigate_recursive(&self, path: &Path) {
        let changed = self.set_recursive_flag(true);
        if normalize(path) == self.folder() {
            if changed {
                self.reload();
            }
        } else {
            self.navigate(path, true, true);
        }
        self.emit(TabEvent::Changed);
    }

    /// Update the recursive flag and presentation; true if it changed.
    fn set_recursive_flag(&self, on: bool) -> bool {
        if self.0.recursive.get() == on {
            return false;
        }
        self.0.recursive.set(on);
        self.0.cfg.show_folders.set(on && settings().boolean("show-folder-labels"));
        self.0.cfg.refresh_cells();
        self.0.list.set_folder_column_visible(on);
        self.0.waterfall.config_changed();
        if !on && settings().string("sort-key") == "folder" {
            let _ = settings().set_string("sort-key", "name");
        }
        true
    }

    // ----- loading -----

    pub fn reload(&self) {
        let i = &self.0;
        if let Some(c) = i.scan_cancel.borrow_mut().take() {
            c.cancel();
        }
        if let Some(c) = i.dims_cancel.borrow_mut().take() {
            c.cancel();
        }
        if let Some(c) = i.ratings_cancel.borrow_mut().take() {
            c.cancel();
        }
        i.viewer.close();
        i.model.clear();
        i.truncated.set(false);
        i.expect_items.set(false);
        *i.scan_error.borrow_mut() = None;
        i.scanning.set(true);
        self.update_page();
        self.watch_folder();

        let s = settings();
        let opts = ScanOptions {
            recursive: i.recursive.get(),
            show_hidden: s.boolean("show-hidden"),
            same_device: s.boolean("same-device"),
            cap: if i.recursive.get() { s.uint("recursive-cap") as usize } else { usize::MAX },
            videos: s.boolean("show-videos") && crate::decode::video::available(),
        };
        let cancel = Cancel::default();
        *i.scan_cancel.borrow_mut() = Some(cancel.clone());
        let started = Instant::now();
        let rx = scan::spawn_scan(self.folder(), opts, cancel.clone());
        let w = self.downgrade();
        glib::spawn_future_local(async move {
            let mut first = true;
            while let Ok(msg) = rx.recv().await {
                if cancel.is_cancelled() {
                    return;
                }
                // After the first paint, merge everything that queued up so the
                // views see a few large updates instead of many small ones.
                let msg = match msg {
                    ScanMsg::Batch(mut entries) if !first => {
                        let mut tail = None;
                        while let Ok(next) = rx.try_recv() {
                            match next {
                                ScanMsg::Batch(more) => entries.extend(more),
                                other => {
                                    tail = Some(other);
                                    break;
                                }
                            }
                        }
                        if let Some(t) = upgrade(&w) {
                            let (n, t0) = (entries.len(), Instant::now());
                            t.0.model.extend(std::mem::take(&mut entries));
                            tracing::trace!("appended {n} items (coalesced) in {:?}", t0.elapsed());
                            t.apply_pending_select();
                            t.emit(TabEvent::Changed);
                        }
                        match tail {
                            Some(m) => m,
                            None => {
                                glib::timeout_future(Duration::from_millis(250)).await;
                                continue;
                            }
                        }
                    }
                    m => m,
                };
                let Some(t) = upgrade(&w) else { return };
                match msg {
                    ScanMsg::Batch(entries) => {
                        if first {
                            // Give the view a size before it sees any items;
                            // an unallocated GtkGridView builds a widget for
                            // every item it cannot prove is off-screen.
                            t.0.expect_items.set(true);
                            t.update_page();
                            let view = t.view_widget();
                            for _ in 0..40 {
                                if view.is_mapped() && view.height() > 0 {
                                    break;
                                }
                                glib::timeout_future(Duration::from_millis(5)).await;
                            }
                            if cancel.is_cancelled() {
                                return;
                            }
                        }
                        let n = entries.len();
                        let t0 = Instant::now();
                        t.0.model.extend(entries);
                        tracing::trace!(
                            "appended {n} items in {:?} (cells created so far: {})",
                            t0.elapsed(),
                            crate::browser::cell::CELLS_CREATED.load(std::sync::atomic::Ordering::Relaxed)
                        );
                        if first {
                            first = false;
                            tracing::debug!("first batch after {:?}", started.elapsed());
                            if t.0.focus_after_load.get() && t.0.pending_select.borrow().is_none() {
                                // Once the view has laid out its first cells.
                                let w = t.downgrade();
                                glib::idle_add_local_once(move || {
                                    if let Some(t) = upgrade(&w) {
                                        if !text_entry_has_focus(&t.0.root) && t.0.model.first_selected().is_none() {
                                            t.focus_first_item();
                                        }
                                    }
                                });
                            }
                        }
                        t.apply_pending_select();
                        t.emit(TabEvent::Changed);
                    }
                    ScanMsg::Done { total, truncated } => {
                        tracing::debug!("scan done: {total} files in {:?}", started.elapsed());
                        t.0.scanning.set(false);
                        t.0.truncated.set(truncated);
                        if truncated {
                            t.toast_text(&format!("{} {} {}", tr("Showing first"), format_count(total), tr("files")));
                        }
                        t.0.scan_cancel.borrow_mut().take();
                        t.update_page();
                        if total == 0 && t.0.focus_after_load.get() && !text_entry_has_focus(&t.0.root) {
                            if let Some(b) = t.0.empty_page.child() {
                                b.grab_focus();
                            }
                        }
                        t.apply_pending_select();
                        if t.0.open_first_after_load.take() {
                            if t.0.model.n_items() > 0 {
                                let w = t.downgrade();
                                t.0.model.when_sorted(move || {
                                    if let Some(t) = upgrade(&w) {
                                        if t.0.model.n_items() > 0 && !t.viewer_open() {
                                            t.open_viewer_at(0);
                                        }
                                    }
                                });
                            } else {
                                t.toast_text(&tr("No images in this folder"));
                            }
                        }
                        t.after_scan();
                        t.emit(TabEvent::Changed);
                    }
                    ScanMsg::Error(e) => {
                        t.0.open_first_after_load.set(false);
                        t.0.scanning.set(false);
                        *t.0.scan_error.borrow_mut() = Some(e);
                        t.update_page();
                        t.emit(TabEvent::Changed);
                    }
                }
            }
        });
        self.emit(TabEvent::Changed);
    }

    /// Load star ratings (xattrs) on a worker when the filter needs them.
    fn ensure_ratings(&self) {
        if !self.0.model.needs_ratings() || self.0.ratings_cancel.borrow().is_some() {
            return;
        }
        let items: Vec<ImageItem> = self.0.model.all_items().into_iter().filter(|i| i.rating().is_none()).collect();
        if items.is_empty() {
            return;
        }
        let paths: Vec<PathBuf> = items.iter().map(|i| i.path()).collect();
        let cancel = Cancel::default();
        *self.0.ratings_cancel.borrow_mut() = Some(cancel.clone());
        let (tx, rx) = async_channel::unbounded::<Vec<(usize, u8)>>();
        let c = cancel.clone();
        std::thread::spawn(move || {
            for (ci, chunk) in paths.chunks(512).enumerate() {
                if c.is_cancelled() {
                    return;
                }
                let batch =
                    chunk.iter().enumerate().map(|(j, p)| (ci * 512 + j, crate::fs::xattrs::read_rating(p))).collect();
                if tx.send_blocking(batch).is_err() {
                    return;
                }
            }
        });
        let w = self.downgrade();
        glib::spawn_future_local(async move {
            while let Ok(batch) = rx.recv().await {
                if cancel.is_cancelled() {
                    return;
                }
                for (idx, r) in batch {
                    if let Some(item) = items.get(idx) {
                        item.set_rating(Some(r));
                    }
                }
            }
            if cancel.is_cancelled() {
                return;
            }
            if let Some(t) = upgrade(&w) {
                t.0.ratings_cancel.borrow_mut().take();
                t.0.model.refilter();
                t.emit(TabEvent::Changed);
            }
        });
    }

    /// Diagnostics (`FLOWLIN_DEBUG_GRID=1`): how many cells the grid holds.
    fn debug_grid(&self) {
        if std::env::var_os("FLOWLIN_DEBUG_GRID").is_none() {
            return;
        }
        let grid = self.0.grid.clone();
        glib::timeout_add_local_once(Duration::from_millis(800), move || {
            let (mut n, mut mapped, mut sample) = (0, 0, Vec::new());
            let mut c = grid.first_child();
            while let Some(w) = c {
                n += 1;
                if w.is_child_visible() && w.is_mapped() {
                    mapped += 1;
                }
                if sample.len() < 4 {
                    sample.push(format!("{}:{}x{}", w.css_name(), w.width(), w.height()));
                }
                c = w.next_sibling();
            }
            tracing::warn!(
                "grid: {n} children ({mapped} mapped), grid {}x{}, sample {:?}",
                grid.width(),
                grid.height(),
                sample
            );
        });
    }

    fn after_scan(&self) {
        self.debug_grid();
        self.ensure_ratings();
        let mode = self.0.mode.get();
        let key = self.0.model.sort_state().key;
        if mode == ViewMode::Waterfall || key == SortKey::Dimensions {
            self.ensure_dimensions();
        }
    }

    /// Read image headers for items without known dimensions (waterfall
    /// layout and dimension sort need them).
    fn ensure_dimensions(&self) {
        if self.0.dims_cancel.borrow().is_some() || self.0.scanning.get() {
            return;
        }
        let items: Vec<ImageItem> = self.0.model.all_items().into_iter().filter(|i| i.dimensions().is_none()).collect();
        if items.is_empty() {
            return;
        }
        let paths: Vec<PathBuf> = items.iter().map(|i| i.path()).collect();
        let cancel = Cancel::default();
        *self.0.dims_cancel.borrow_mut() = Some(cancel.clone());
        let (tx, rx) = async_channel::unbounded::<Vec<(usize, u32, u32)>>();
        let c = cancel.clone();
        std::thread::spawn(move || {
            use rayon::prelude::*;
            for (ci, chunk) in paths.chunks(256).enumerate() {
                if c.is_cancelled() {
                    return;
                }
                let batch: Vec<(usize, u32, u32)> = chunk
                    .par_iter()
                    .enumerate()
                    .filter_map(|(j, p)| crate::decode::dimensions(p).map(|(w, h)| (ci * 256 + j, w, h)))
                    .collect();
                if tx.send_blocking(batch).is_err() {
                    return;
                }
            }
        });
        let w = self.downgrade();
        glib::spawn_future_local(async move {
            while let Ok(batch) = rx.recv().await {
                if cancel.is_cancelled() {
                    return;
                }
                let Some(t) = upgrade(&w) else { return };
                for (idx, wd, ht) in batch {
                    if let Some(item) = items.get(idx) {
                        item.set_dimensions(wd, ht);
                    }
                }
                if t.0.mode.get() == ViewMode::Waterfall {
                    t.0.waterfall.invalidate();
                }
            }
            if cancel.is_cancelled() {
                return; // a newer job owns dims_cancel now
            }
            if let Some(t) = upgrade(&w) {
                t.0.dims_cancel.borrow_mut().take();
                if t.0.model.sort_state().key == SortKey::Dimensions {
                    t.0.model.resort();
                }
            }
        });
    }

    fn apply_pending_select(&self) {
        let pending = self.0.pending_select.borrow().clone();
        let Some((path, open)) = pending else { return };
        let Some(pos) = self.0.model.position_of_path(&path) else {
            if !self.0.scanning.get() {
                self.0.pending_select.borrow_mut().take();
            }
            return;
        };
        self.0.pending_select.borrow_mut().take();
        self.select_and_reveal(pos);
        if open {
            self.open_viewer_at(pos);
        }
    }

    fn update_page(&self) {
        let i = &self.0;
        let page = if let Some(e) = i.scan_error.borrow().as_ref() {
            i.error_page.set_description(Some(&glib::markup_escape_text(e)));
            "error"
        } else if i.model.total() > 0 || i.expect_items.get() {
            i.mode.get().id()
        } else if i.scanning.get() {
            "loading"
        } else {
            let recursive = i.recursive.get();
            i.empty_page.set_title(&if recursive {
                tr("No Images Here or in Subfolders")
            } else {
                tr("No Images in This Folder")
            });
            if let Some(c) = i.empty_page.child() {
                c.set_visible(!recursive);
            }
            "empty"
        };
        if i.stack.visible_child_name().as_deref() != Some(page) {
            i.stack.set_visible_child_name(page);
        }
    }

    // ----- file monitor -----

    fn watch_folder(&self) {
        if let Some(m) = self.0.monitor.borrow_mut().take() {
            m.cancel();
        }
        let file = gio::File::for_path(self.folder());
        let Ok(mon) = file.monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE) else {
            return;
        };
        mon.set_rate_limit(500);
        let w = self.downgrade();
        mon.connect_changed(move |_, file, other, event| {
            let Some(t) = upgrade(&w) else { return };
            let Some(path) = file.path() else { return };
            use gio::FileMonitorEvent as E;
            match event {
                E::Created | E::MovedIn => t.file_added(&path),
                E::Deleted | E::MovedOut => t.file_removed(&path),
                E::Renamed => {
                    t.file_removed(&path);
                    if let Some(new) = other.and_then(|o| o.path()) {
                        t.file_added(&new);
                    }
                }
                E::ChangesDoneHint => t.file_changed(&path),
                // Our own rating writes land here too: only refresh the rating.
                E::AttributeChanged => t.refresh_rating(&path),
                _ => {}
            }
        });
        *self.0.monitor.borrow_mut() = Some(mon);
    }

    fn file_added(&self, path: &Path) {
        let folder = self.folder();
        let belongs =
            if self.is_recursive() { path.starts_with(&folder) } else { path.parent() == Some(folder.as_path()) };
        if !belongs {
            return;
        }
        if path.is_dir() {
            self.emit(TabEvent::FoldersChanged(self.folder()));
            return;
        }
        if !settings().boolean("show-hidden") && crate::util::is_hidden_name(&crate::util::file_name(path)) {
            return;
        }
        let videos = settings().boolean("show-videos") && crate::decode::video::available();
        if !crate::fs::formats::is_media(path, true, videos) {
            return;
        }
        if let Some(item) = self.0.model.get(path) {
            self.file_changed(&item.path());
            return;
        }
        if let Some(e) = FileEntry::from_path(path, &self.folder()) {
            self.0.model.extend(vec![e]);
            self.update_page();
            self.emit(TabEvent::Changed);
        }
    }

    fn file_removed(&self, path: &Path) {
        if self.0.model.remove(path).is_some() {
            crate::thumbs::ThumbService::get().invalidate(path);
            self.update_page();
            self.emit(TabEvent::Changed);
        } else {
            self.emit(TabEvent::FoldersChanged(self.folder()));
        }
    }

    /// The file's content changed on disk: reload its thumbnail if the
    /// size or modification time actually differ.
    fn file_changed(&self, path: &Path) {
        let Some(item) = self.0.model.get(path) else { return };
        let Some(e) = FileEntry::from_path(path, &self.folder()) else { return };
        if e.size == item.size() && e.mtime == item.mtime() && item.thumb_state() != ThumbState::Failed {
            return;
        }
        item.update_stat(&e);
        let thumbs = crate::thumbs::ThumbService::get();
        thumbs.invalidate(path);
        self.0.viewer.forget(path);
        item.set_texture(None);
        item.set_thumb_state(ThumbState::None);
        if item.bound() > 0 {
            thumbs.request(&item, self.0.cfg.thumb_px());
        }
        self.0.model.item_changed(&item);
    }

    /// Re-read a file's rating off the main thread.
    fn refresh_rating(&self, path: &Path) {
        let Some(item) = self.0.model.get(path) else { return };
        let p = path.to_path_buf();
        glib::spawn_future_local(async move {
            if let Ok(r) = gio::spawn_blocking(move || crate::fs::xattrs::read_rating(&p)).await {
                item.set_rating(Some(r));
            }
        });
    }

    // ----- views -----

    pub fn view_mode(&self) -> ViewMode {
        self.0.mode.get()
    }

    /// Only the visible view is connected to the model: hidden list views
    /// never get a size and would otherwise build widgets for thousands of
    /// items.
    fn attach_model(&self) {
        let i = &self.0;
        let sel = &i.model.selection;
        let mode = i.mode.get();
        i.grid.set_model((mode == ViewMode::Grid).then_some(sel));
        i.list.view.set_model((mode == ViewMode::List).then_some(sel));
        i.waterfall.set_active(mode == ViewMode::Waterfall);
    }

    pub fn set_view_mode(&self, mode: ViewMode) {
        if self.0.mode.get() == mode {
            return;
        }
        let had_focus = self.view_has_focus();
        self.0.mode.set(mode);
        self.attach_model();
        self.update_page();
        if mode == ViewMode::Waterfall {
            self.0.waterfall.invalidate();
            self.ensure_dimensions();
        }
        if let Some(pos) = self.0.model.first_selected() {
            self.scroll_to(pos, had_focus);
        } else if had_focus {
            self.focus_view();
        }
    }

    fn view_widget(&self) -> gtk::Widget {
        match self.0.mode.get() {
            ViewMode::Grid => self.0.grid.clone().upcast(),
            ViewMode::Waterfall => self.0.waterfall.clone().upcast(),
            ViewMode::List => self.0.list.view.clone().upcast(),
        }
    }

    fn view_has_focus(&self) -> bool {
        let w = self.view_widget();
        w.root().and_then(|r| r.focus()).is_some_and(|f| f == w || f.is_ancestor(&w))
    }

    pub fn focus_view(&self) {
        match self.0.mode.get() {
            ViewMode::Grid => {
                self.0.grid.grab_focus();
            }
            ViewMode::Waterfall => {
                self.0.waterfall.grab_focus();
            }
            ViewMode::List => {
                self.0.list.view.grab_focus();
            }
        }
    }

    /// Number of columns the grid currently shows (from its laid-out cells).
    fn grid_columns(&self) -> u32 {
        let grid = &self.0.grid;
        let mut rows: std::collections::HashMap<i32, u32> = Default::default();
        let mut child = grid.first_child();
        while let Some(c) = child {
            if c.is_visible() && c.css_name() == "child" {
                if let Some(b) = c.compute_bounds(grid) {
                    *rows.entry(b.y().round() as i32).or_default() += 1;
                }
            }
            child = c.next_sibling();
        }
        rows.values().copied().max().unwrap_or(1).max(1)
    }

    /// The item one row above (`rows < 0`) or below `pos` in the current
    /// layout; used by Up/Down in the viewer.
    pub fn row_neighbour(&self, pos: u32, rows: i32) -> Option<u32> {
        let n = self.0.model.n_items();
        if n == 0 {
            return None;
        }
        match self.0.mode.get() {
            ViewMode::Grid => {
                let cols = self.grid_columns() as i64;
                let target = pos as i64 + rows as i64 * cols;
                (0..n as i64).contains(&target).then_some(target as u32)
            }
            ViewMode::List => {
                let target = pos as i64 + rows as i64;
                (0..n as i64).contains(&target).then_some(target as u32)
            }
            ViewMode::Waterfall => self.0.waterfall.neighbour(pos, 0, rows.signum()),
        }
    }

    /// Focus (not select) the first cell so the first arrow press moves.
    fn focus_first_item(&self) {
        if self.0.model.n_items() == 0 {
            return; // e.g. a deferred call after a quick folder change
        }
        match self.0.mode.get() {
            ViewMode::Grid => self.0.grid.scroll_to(0, gtk::ListScrollFlags::FOCUS, None),
            ViewMode::List => {
                self.0.list.view.scroll_to(0, None::<&gtk::ColumnViewColumn>, gtk::ListScrollFlags::FOCUS, None)
            }
            ViewMode::Waterfall => {
                self.0.waterfall.set_cursor(0, false, false);
                self.0.waterfall.grab_focus();
            }
        }
    }

    fn scroll_to(&self, pos: u32, focus: bool) {
        if pos >= self.0.model.n_items() {
            return;
        }
        let flags = if focus { gtk::ListScrollFlags::FOCUS } else { gtk::ListScrollFlags::NONE };
        match self.0.mode.get() {
            ViewMode::Grid => self.0.grid.scroll_to(pos, flags, None),
            ViewMode::List => self.0.list.view.scroll_to(pos, None::<&gtk::ColumnViewColumn>, flags, None),
            ViewMode::Waterfall => {
                self.0.waterfall.set_cursor(pos, false, false);
                if focus {
                    self.0.waterfall.grab_focus();
                }
            }
        }
    }

    pub fn select_and_reveal(&self, pos: u32) {
        self.0.model.select_only(pos);
        self.scroll_to(pos, true);
    }

    pub fn set_thumb_size(&self, size: i32) {
        self.0.cfg.size.set(size);
        if let Some(sw) = self.0.grid.parent().and_downcast::<gtk::ScrolledWindow>() {
            grid::fit_columns(&self.0.grid, sw.hadjustment().page_size(), &self.0.cfg);
        }
        self.0.cfg.refresh_cells();
        self.0.waterfall.config_changed();
        self.0.grid.queue_resize();
    }

    pub fn set_show_names(&self, show: bool) {
        self.0.cfg.show_names.set(show);
        self.0.cfg.refresh_cells();
        self.0.waterfall.config_changed();
    }

    pub fn set_show_folder_labels(&self, show: bool) {
        self.0.cfg.show_folders.set(show && self.is_recursive());
        self.0.cfg.refresh_cells();
        self.0.waterfall.config_changed();
    }

    pub fn set_sort(&self, key: SortKey, desc: bool) {
        self.0.model.set_sort(key, desc);
        self.0.list.show_sort(key, desc);
        if key == SortKey::Dimensions {
            self.ensure_dimensions();
        }
        if let Some(pos) = self.0.model.first_selected() {
            self.scroll_to(pos, false);
        }
    }

    // ----- viewer -----

    pub fn open_viewer(&self) {
        let pos = self.0.model.first_selected().unwrap_or(0);
        if self.0.model.n_items() > 0 {
            self.open_viewer_at(pos);
        }
    }

    pub fn open_viewer_at(&self, pos: u32) {
        self.0.viewer.open(pos);
        self.emit(TabEvent::ViewerOpened);
        self.emit(TabEvent::Changed);
    }

    pub fn close_viewer(&self) {
        self.0.viewer.close();
    }

    // ----- filter -----

    pub fn toggle_filter(&self) {
        let bar = &self.0.search_bar;
        if bar.is_search_mode() && self.0.search_entry.has_focus() {
            self.0.search_entry.set_text("");
            bar.set_search_mode(false);
            self.focus_view();
        } else {
            bar.set_search_mode(true);
            self.0.search_entry.grab_focus();
        }
    }

    // ----- selection helpers -----

    pub fn selected_paths(&self) -> Vec<PathBuf> {
        if self.viewer_open() {
            return self.0.viewer.current_path().into_iter().collect();
        }
        self.0.model.selected_paths()
    }

    pub fn selected_items(&self) -> Vec<ImageItem> {
        if self.viewer_open() {
            return self.0.viewer.current().into_iter().collect();
        }
        self.0.model.selected_items()
    }

    // ----- file operations -----

    pub fn trash_selected(&self) {
        let paths = self.selected_paths();
        self.trash(paths);
    }

    pub fn trash(&self, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        let w = self.downgrade();
        glib::spawn_future_local(async move {
            let (done, errs) = ops::trash(&paths).await;
            let Some(t) = upgrade(&w) else { return };
            // Remember where the selection was so the next item gets selected.
            let first = t.0.model.first_selected();
            for p in &done {
                t.0.model.remove(p);
                t.0.viewer.forget(p);
            }
            t.update_page();
            t.emit(TabEvent::Changed);
            if !t.viewer_open() {
                if let Some(pos) = first {
                    let n = t.0.model.n_items();
                    if n > 0 {
                        t.select_and_reveal(pos.min(n - 1));
                    }
                }
            }
            if let Some(e) = errs.first() {
                t.toast_text(&format!("{}: {}", tr("Could not move to trash"), e.message()));
            }
            if done.is_empty() {
                return;
            }
            let msg = if done.len() == 1 {
                format!("“{}” {}", crate::util::file_name(&done[0]), tr("moved to trash"))
            } else {
                format!("{} {}", done.len(), tr("items moved to trash"))
            };
            let toast = adw::Toast::new(&msg);
            toast.set_button_label(Some(&tr("Undo")));
            toast.set_timeout(10);
            let w2 = t.downgrade();
            toast.connect_button_clicked(move |_| {
                let done = done.clone();
                let w3 = w2.clone();
                glib::spawn_future_local(async move {
                    let n = ops::restore_from_trash(&done).await;
                    if let Some(t) = upgrade(&w3) {
                        for p in &done {
                            t.file_added(p);
                        }
                        if n < done.len() {
                            t.toast_text(&tr("Some files could not be restored"));
                        }
                    }
                });
            });
            t.toast(toast);
        });
    }

    pub fn delete_selected_permanently(&self, parent: &gtk::Window) {
        let paths = self.selected_paths();
        if paths.is_empty() {
            return;
        }
        let heading = if paths.len() == 1 {
            format!("{} “{}”?", tr("Permanently delete"), crate::util::file_name(&paths[0]))
        } else {
            format!("{} {} {}?", tr("Permanently delete"), paths.len(), tr("items"))
        };
        let dialog = adw::AlertDialog::new(Some(&heading), Some(&tr("This cannot be undone.")));
        dialog.add_response("cancel", &tr("Cancel"));
        dialog.add_response("delete", &tr("Delete"));
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        let w = self.downgrade();
        dialog.connect_response(None, move |_, r| {
            if r != "delete" {
                return;
            }
            let paths = paths.clone();
            let w = w.clone();
            glib::spawn_future_local(async move {
                let (done, errs) = ops::delete_permanently(&paths).await;
                let Some(t) = upgrade(&w) else { return };
                for p in &done {
                    t.0.model.remove(p);
                }
                t.update_page();
                t.emit(TabEvent::Changed);
                if let Some(e) = errs.first() {
                    t.toast_text(e.message());
                }
            });
        });
        dialog.present(Some(parent));
    }

    pub fn rename_selected(&self, parent: &gtk::Window) {
        let Some(path) = self.selected_paths().into_iter().next() else { return };
        let w = self.downgrade();
        rename_dialog(parent, &path, move |old, new| {
            if let Some(t) = upgrade(&w) {
                t.0.model.renamed(old, new);
                t.0.viewer.forget(old);
            }
        });
    }

    /// Rate the selection (xattrs are written on a worker).
    pub fn set_rating(&self, stars: u8) {
        let items = self.selected_items();
        if items.is_empty() {
            return;
        }
        let paths: Vec<PathBuf> = items.iter().map(|i| i.path()).collect();
        let w = self.downgrade();
        glib::spawn_future_local(async move {
            let res = gio::spawn_blocking(move || {
                paths
                    .iter()
                    .map(|p| crate::fs::xattrs::write_rating(p, stars).map_err(|e| e.to_string()))
                    .collect::<Vec<_>>()
            })
            .await;
            let Some(t) = upgrade(&w) else { return };
            let Ok(results) = res else { return };
            let mut ok = 0;
            for (item, r) in items.iter().zip(results) {
                match r {
                    Ok(()) => {
                        item.set_rating(Some(stars));
                        t.0.model.item_changed(item);
                        ok += 1;
                    }
                    Err(e) => {
                        if !t.0.xattr_warned.replace(true) {
                            t.toast_text(&format!("{}: {e}", tr("Ratings are not supported on this file system")));
                        }
                        break;
                    }
                }
            }
            if ok > 0 {
                t.toast_text(&if stars == 0 {
                    tr("Rating cleared")
                } else {
                    format!("{} {}", tr("Rated"), "★".repeat(stars as usize))
                });
            }
        });
    }

    /// Put the selection on the clipboard (Nautilus-compatible).
    pub fn copy_to_clipboard(&self, cut: bool) {
        let paths = self.selected_paths();
        if paths.is_empty() {
            return;
        }
        let uris: Vec<String> = paths.iter().map(|p| gio::File::for_path(p).uri().to_string()).collect();
        let gnome = format!("{}\n{}", if cut { "cut" } else { "copy" }, uris.join("\n"));
        let uri_list = uris.join("\r\n") + "\r\n";
        let text = paths.iter().map(|p| p.to_string_lossy().into_owned()).collect::<Vec<_>>().join("\n");
        let providers = [
            gdk::ContentProvider::for_bytes(
                "x-special/gnome-copied-files",
                &glib::Bytes::from_owned(gnome.into_bytes()),
            ),
            gdk::ContentProvider::for_bytes("text/uri-list", &glib::Bytes::from_owned(uri_list.into_bytes())),
            gdk::ContentProvider::for_bytes("text/plain;charset=utf-8", &glib::Bytes::from_owned(text.into_bytes())),
        ];
        let union = gdk::ContentProvider::new_union(&providers);
        if let Some(d) = gdk::Display::default() {
            let _ = d.clipboard().set_content(Some(&union));
        }
        let n = paths.len();
        self.toast_text(&if cut {
            format!("{} {}", n, trn("item cut", "items cut", n as u32))
        } else {
            format!("{} {}", n, trn("item copied", "items copied", n as u32))
        });
    }

    pub fn copy_path(&self) {
        let paths = self.selected_paths();
        let text = if paths.is_empty() {
            self.folder().to_string_lossy().into_owned()
        } else {
            paths.iter().map(|p| p.to_string_lossy().into_owned()).collect::<Vec<_>>().join("\n")
        };
        if let Some(d) = gdk::Display::default() {
            d.clipboard().set_text(&text);
        }
    }

    pub fn paste(&self) {
        let Some(d) = gdk::Display::default() else { return };
        let clip = d.clipboard();
        let w = self.downgrade();
        glib::spawn_future_local(async move {
            let res =
                clip.read_future(&["x-special/gnome-copied-files", "text/uri-list"], glib::Priority::DEFAULT).await;
            let Ok((stream, mime)) = res else { return };
            let mut buf = Vec::new();
            loop {
                match stream.read_bytes_future(65536, glib::Priority::DEFAULT).await {
                    Ok(b) if !b.is_empty() => buf.extend_from_slice(&b),
                    _ => break,
                }
            }
            let text = String::from_utf8_lossy(&buf);
            let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#'));
            let mut mode = Transfer::Copy;
            if mime == "x-special/gnome-copied-files" && lines.next() == Some("cut") {
                mode = Transfer::Move;
            }
            let paths: Vec<PathBuf> = lines.filter_map(|u| gio::File::for_uri(u).path()).collect();
            let Some(t) = upgrade(&w) else { return };
            t.transfer_into(paths, &t.folder(), mode).await;
        });
    }

    /// Copy/move files into `dest`, with an undo toast.
    pub async fn transfer_into(&self, paths: Vec<PathBuf>, dest: &Path, mode: Transfer) {
        if paths.is_empty() {
            return;
        }
        let (done, errs) = ops::transfer(&paths, dest, mode).await;
        if let Some(e) = errs.first() {
            self.toast_text(e.message());
        }
        if done.is_empty() {
            return;
        }
        for (src, dst) in &done {
            if mode == Transfer::Move {
                self.file_removed(src);
            }
            if dst.parent() == Some(self.folder().as_path()) {
                self.file_added(dst);
            }
        }
        let n = done.len();
        let msg = match mode {
            Transfer::Copy => format!("{} {}", n, trn("item copied", "items copied", n as u32)),
            Transfer::Move => format!("{} {}", n, trn("item moved", "items moved", n as u32)),
        };
        let toast = adw::Toast::new(&msg);
        toast.set_button_label(Some(&tr("Undo")));
        toast.set_timeout(10);
        toast.connect_button_clicked(move |_| {
            let done = done.clone();
            glib::spawn_future_local(async move {
                match mode {
                    Transfer::Copy => {
                        let copies: Vec<PathBuf> = done.iter().map(|(_, d)| d.clone()).collect();
                        let _ = ops::trash(&copies).await;
                    }
                    Transfer::Move => {
                        for (src, dst) in &done {
                            let (fut, _) = gio::File::for_path(dst).move_future(
                                &gio::File::for_path(src),
                                gio::FileCopyFlags::NOFOLLOW_SYMLINKS,
                                glib::Priority::DEFAULT,
                            );
                            let _ = fut.await;
                        }
                    }
                }
            });
        });
        self.toast(toast);
    }

    pub fn new_folder(&self, parent: &gtk::Window) {
        let dir = self.folder();
        let w = self.downgrade();
        name_dialog(parent, &tr("New Folder"), &tr("Create"), &tr("New Folder"), None, move |name| {
            let dir = dir.clone();
            let w = w.clone();
            glib::spawn_future_local(async move {
                let res = ops::new_folder(&dir, &name).await;
                let Some(t) = upgrade(&w) else { return };
                match res {
                    Ok(_) => t.emit(TabEvent::FoldersChanged(dir)),
                    Err(e) => t.toast_text(e.message()),
                }
            });
        });
    }

    // ----- setup -----

    fn setup(&self) {
        let i = &self.0;
        let w = self.downgrade();
        i.grid.connect_activate(move |_, pos| {
            if let Some(t) = upgrade(&w) {
                t.open_viewer_at(pos);
            }
        });
        let w = self.downgrade();
        i.list.view.connect_activate(move |_, pos| {
            if let Some(t) = upgrade(&w) {
                t.open_viewer_at(pos);
            }
        });
        let w = self.downgrade();
        i.waterfall.connect_activate(move |_, pos| {
            if let Some(t) = upgrade(&w) {
                t.open_viewer_at(pos);
            }
        });

        let w = self.downgrade();
        i.viewer.connect_close(move |item| {
            let Some(t) = upgrade(&w) else { return };
            t.emit(TabEvent::ViewerClosed);
            t.emit(TabEvent::Changed);
            // After the window restored its bars/sidebar. The viewer may have
            // closed because we navigated away: look the item up then.
            let item = item.clone();
            let w = t.downgrade();
            glib::idle_add_local_once(move || {
                let Some(t) = upgrade(&w) else { return };
                if text_entry_has_focus(&t.0.root) {
                    return;
                }
                t.focus_view();
                if let Some(pos) = t.0.model.position_of(&item) {
                    t.select_and_reveal(pos);
                }
            });
        });
        let w = self.downgrade();
        i.viewer.connect_row_step(move |pos, rows| upgrade(&w).and_then(|t| t.row_neighbour(pos, rows)));
        let w = self.downgrade();
        i.viewer.connect_show(move |pos| {
            // Keep the thumbnails behind the viewer on the image being shown.
            if let Some(t) = upgrade(&w) {
                t.0.model.select_only(pos);
                t.scroll_to(pos, false);
            }
        });
        let w = self.downgrade();
        i.viewer.connect_trash(move |item| {
            if let Some(t) = upgrade(&w) {
                t.trash(vec![item.path()]);
            }
        });
        let w = self.downgrade();
        i.viewer.connect_rate(move |_, stars| {
            // selected_items() yields the viewer's current image while it is open.
            if let Some(t) = upgrade(&w) {
                t.set_rating(stars);
            }
        });

        let w = self.downgrade();
        i.search_entry.connect_search_changed(move |e| {
            if let Some(t) = upgrade(&w) {
                t.0.model.set_filter(&e.text());
                t.ensure_ratings();
                t.emit(TabEvent::Changed);
            }
        });
        let w = self.downgrade();
        i.search_entry.connect_stop_search(move |e| {
            e.set_text("");
            if let Some(t) = upgrade(&w) {
                t.0.search_bar.set_search_mode(false);
                t.focus_view();
            }
        });
        let w = self.downgrade();
        i.search_entry.connect_activate(move |_| {
            if let Some(t) = upgrade(&w) {
                if t.0.model.n_items() > 0 {
                    t.select_and_reveal(0);
                }
            }
        });

        for view in [i.grid.clone().upcast::<gtk::Widget>(), i.waterfall.clone().upcast(), i.list.view.clone().upcast()]
        {
            self.setup_view_input(&view);
        }
        // Letters, space, backspace and digits, before the views see them.
        // On the stack so they also work on the empty/error pages.
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let w = self.downgrade();
        keys.connect_key_pressed(move |_, key, _, state| {
            let Some(t) = upgrade(&w) else { return glib::Propagation::Proceed };
            t.handle_view_key(key, state)
        });
        i.stack.add_controller(keys);
        let w = self.downgrade();
        i.model.selection.connect_items_changed(move |_, _, _, _| {
            if let Some(t) = upgrade(&w) {
                t.update_page();
            }
        });
        let w = self.downgrade();
        i.root.connect_scale_factor_notify(move |r| {
            if let Some(t) = upgrade(&w) {
                t.0.cfg.scale.set(r.scale_factor());
                t.0.cfg.refresh_cells();
            }
        });
        i.cfg.scale.set(i.root.scale_factor().max(1));
        self.attach_model();
        i.list.set_folder_column_visible(false);
        let st = i.model.sort_state();
        i.list.show_sort(st.key, st.descending);

        // Drops from other apps/tabs: folders open, files are copied here.
        let drop = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY | gdk::DragAction::MOVE);
        let w = self.downgrade();
        drop.connect_drop(move |target, value, _, _| {
            let Some(t) = upgrade(&w) else { return false };
            let Ok(list) = value.get::<gdk::FileList>() else { return false };
            let paths: Vec<PathBuf> = list.files().iter().filter_map(|f| f.path()).collect();
            if paths.len() == 1 && paths[0].is_dir() {
                t.navigate(&paths[0], true, true);
                return true;
            }
            let dest = t.folder();
            if paths.iter().all(|p| p.parent() == Some(dest.as_path())) {
                return false; // dropped onto its own folder
            }
            let mode = if target.current_drop().is_some_and(|d| d.actions().contains(gdk::DragAction::MOVE))
                && target.current_event_state().contains(gdk::ModifierType::SHIFT_MASK)
            {
                Transfer::Move
            } else {
                Transfer::Copy
            };
            glib::spawn_future_local(async move {
                t.transfer_into(paths, &dest, mode).await;
            });
            true
        });
        i.stack.add_controller(drop);
    }

    fn item_at(view: &gtk::Widget, x: f64, y: f64) -> Option<ImageItem> {
        let mut w = view.pick(x, y, gtk::PickFlags::DEFAULT);
        while let Some(cur) = w {
            if let Some(cell) = cur.downcast_ref::<ThumbCell>() {
                return cell.item();
            }
            // ColumnView rows: the name column holds a ThumbCell.
            if cur.css_name() == "row" {
                let mut c = cur.first_child();
                while let Some(cell) = c {
                    if let Some(tc) = find_thumb_cell(&cell) {
                        return tc.item();
                    }
                    c = cell.next_sibling();
                }
            }
            if &cur == view {
                break;
            }
            w = cur.parent();
        }
        None
    }

    fn setup_view_input(&self, view: &gtk::Widget) {
        // Ctrl+scroll resizes thumbnails.
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        scroll.set_propagation_phase(gtk::PropagationPhase::Capture);
        scroll.connect_scroll(|c, _, dy| {
            if !c.current_event_state().contains(gdk::ModifierType::CONTROL_MASK) {
                return glib::Propagation::Proceed;
            }
            let s = settings();
            let cur = s.int("thumbnail-size");
            let next = if dy < 0.0 { cur + 1 } else { cur - 1 };
            let _ = s.set_int("thumbnail-size", next.clamp(0, 4));
            glib::Propagation::Stop
        });
        view.add_controller(scroll);

        // Right button: a short click opens the context menu, a drag of at
        // least GESTURE_MIN px is a navigation gesture.
        let drag = gtk::GestureDrag::new();
        drag.set_button(gdk::BUTTON_SECONDARY);
        let w = self.downgrade();
        let v = view.downgrade();
        drag.connect_drag_begin(move |g, x, y| {
            let (Some(t), Some(view)) = (upgrade(&w), v.upgrade()) else { return };
            g.set_state(gtk::EventSequenceState::Claimed);
            let p = view.compute_point(&t.0.root, &gtk::graphene::Point::new(x as f32, y as f32));
            let p = p.map(|p| (p.x() as f64, p.y() as f64)).unwrap_or((x, y));
            *t.0.trail_points.borrow_mut() = vec![p];
        });
        let w = self.downgrade();
        drag.connect_drag_update(move |_, dx, dy| {
            let Some(t) = upgrade(&w) else { return };
            let mut pts = t.0.trail_points.borrow_mut();
            if let Some(&(x0, y0)) = pts.first() {
                pts.push((x0 + dx, y0 + dy));
            }
            drop(pts);
            if dx.hypot(dy) > 8.0 {
                t.0.trail.queue_draw();
            }
        });
        let w = self.downgrade();
        let v = view.downgrade();
        drag.connect_drag_end(move |g, dx, dy| {
            let (Some(t), Some(view)) = (upgrade(&w), v.upgrade()) else { return };
            t.0.trail_points.borrow_mut().clear();
            t.0.trail.queue_draw();
            match classify_gesture(dx, dy) {
                Some(gesture) => t.run_gesture(gesture),
                None => {
                    let Some((x, y)) = g.start_point() else { return };
                    let item = Tab::item_at(&view, x, y);
                    if let Some(item) = &item {
                        if let Some(pos) = t.0.model.position_of(item) {
                            if !t.0.model.selection.is_selected(pos) {
                                t.0.model.select_only(pos);
                            }
                        }
                    }
                    t.popup_menu(&view, item.is_some(), x, y);
                }
            }
        });
        view.add_controller(drag);

        // Drag files out (to the sidebar, other tabs, other apps).
        let drag = gtk::DragSource::new();
        drag.set_actions(gdk::DragAction::COPY | gdk::DragAction::MOVE);
        let w = self.downgrade();
        let v = view.downgrade();
        drag.connect_prepare(move |_, x, y| {
            let (t, view) = (upgrade(&w)?, v.upgrade()?);
            let item = Tab::item_at(&view, x, y)?;
            let pos = t.0.model.position_of(&item)?;
            if !t.0.model.selection.is_selected(pos) {
                t.0.model.select_only(pos);
            }
            let files: Vec<gio::File> = t.0.model.selected_paths().iter().map(gio::File::for_path).collect();
            Some(gdk::ContentProvider::for_value(&gdk::FileList::from_array(&files).to_value()))
        });
        drag.connect_drag_begin(|src, _| {
            let icon = gtk::IconTheme::for_display(&gdk::Display::default().unwrap()).lookup_icon(
                "image-x-generic-symbolic",
                &[],
                32,
                1,
                gtk::TextDirection::None,
                gtk::IconLookupFlags::empty(),
            );
            src.set_icon(Some(&icon), 16, 16);
        });
        view.add_controller(drag);
    }

    pub fn run_gesture(&self, g: Gesture) {
        match g {
            Gesture::Up => self.go_up(),
            Gesture::Down => self.go_back(),
            Gesture::Left => self.jump_folder(false),
            Gesture::Right => self.jump_folder(true),
            Gesture::UpRight => self.jump_sibling(),
            Gesture::DownRight => self.emit(TabEvent::CloseTab),
        }
    }

    pub fn handle_view_key(&self, key: gdk::Key, state: gdk::ModifierType) -> glib::Propagation {
        use gdk::Key;
        if state
            .intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK | gdk::ModifierType::SUPER_MASK)
        {
            return glib::Propagation::Proceed;
        }
        // Expire the type-ahead buffer after a pause.
        if self.0.typeahead_at.get().is_some_and(|t| t.elapsed() > Duration::from_millis(1000)) {
            self.0.typeahead.borrow_mut().clear();
        }
        let typing = !self.0.typeahead.borrow().is_empty();
        match key {
            Key::space if !typing => {
                self.open_viewer();
                return glib::Propagation::Stop;
            }
            Key::BackSpace => {
                if typing {
                    self.0.typeahead.borrow_mut().pop();
                } else {
                    self.go_up();
                }
                return glib::Propagation::Stop;
            }
            Key::Escape => {
                if typing {
                    self.0.typeahead.borrow_mut().clear();
                } else if self.0.model.is_filtered() {
                    self.0.search_entry.set_text("");
                    self.0.search_bar.set_search_mode(false);
                } else {
                    self.0.model.selection.unselect_all();
                }
                return glib::Propagation::Stop;
            }
            _ => {}
        }
        let Some(ch) = key.to_unicode().filter(|c| !c.is_control()) else { return glib::Propagation::Proceed };
        if !typing {
            if settings().boolean("single-key-navigation") {
                match ch {
                    'w' | 'W' => {
                        self.go_up();
                        return glib::Propagation::Stop;
                    }
                    's' | 'S' => {
                        self.go_back();
                        return glib::Propagation::Stop;
                    }
                    'a' | 'A' => {
                        self.jump_folder(false);
                        return glib::Propagation::Stop;
                    }
                    'd' | 'D' => {
                        self.jump_folder(true);
                        return glib::Propagation::Stop;
                    }
                    'r' | 'R' => {
                        self.set_recursive(!self.is_recursive());
                        self.emit(TabEvent::Changed);
                        return glib::Propagation::Stop;
                    }
                    _ => {}
                }
            }
            if let Some(d) = ch.to_digit(10).filter(|d| *d <= 5) {
                self.set_rating(d as u8);
                return glib::Propagation::Stop;
            }
            if ch == ' ' {
                return glib::Propagation::Proceed;
            }
        }
        self.typeahead_push(ch);
        glib::Propagation::Stop
    }

    fn typeahead_push(&self, ch: char) {
        self.0.typeahead.borrow_mut().extend(ch.to_lowercase());
        self.0.typeahead_at.set(Some(Instant::now()));
        let needle = self.0.typeahead.borrow().clone();
        let m = &self.0.model;
        let n = m.n_items();
        if n == 0 {
            return;
        }
        // A single new letter searches from the next item; a longer prefix
        // re-checks the current one first.
        let start = m.first_selected().map(|p| if needle.chars().count() == 1 { p + 1 } else { p }).unwrap_or(0);
        for k in 0..n {
            let pos = (start + k) % n;
            if let Some(item) = m.item(pos) {
                if item.with_name(|name| name.to_lowercase().starts_with(&needle)) {
                    self.select_and_reveal(pos);
                    return;
                }
            }
        }
    }

    fn popup_menu(&self, view: &gtk::Widget, on_item: bool, x: f64, y: f64) {
        let menu = gio::Menu::new();
        if on_item {
            let s1 = gio::Menu::new();
            s1.append(Some(&tr("Open")), Some("win.open-viewer"));
            s1.append(Some(&tr("Open With…")), Some("win.open-with"));
            s1.append(Some(&tr("Show in File Manager")), Some("win.show-in-fm"));
            menu.append_section(None, &s1);
            let s2 = gio::Menu::new();
            s2.append(Some(&tr("Cut")), Some("win.cut"));
            s2.append(Some(&tr("Copy")), Some("win.copy"));
            s2.append(Some(&tr("Copy Path")), Some("win.copy-path"));
            menu.append_section(None, &s2);
            let s3 = gio::Menu::new();
            s3.append(Some(&tr("Rename…")), Some("win.rename"));
            s3.append(Some(&tr("Move to Trash")), Some("win.trash"));
            menu.append_section(None, &s3);
            let s4 = gio::Menu::new();
            s4.append(Some(&tr("Properties")), Some("win.properties"));
            menu.append_section(None, &s4);
        } else {
            let s1 = gio::Menu::new();
            s1.append(Some(&tr("Paste")), Some("win.paste"));
            s1.append(Some(&tr("New Folder…")), Some("win.new-folder"));
            menu.append_section(None, &s1);
            let s2 = gio::Menu::new();
            s2.append(Some(&tr("Include Subfolders")), Some("win.recursive"));
            s2.append(Some(&tr("Refresh")), Some("win.refresh"));
            s2.append(Some(&tr("Open in File Manager")), Some("win.open-fm"));
            menu.append_section(None, &s2);
        }
        // Parent to the tab root rather than the scrolling view: list views
        // clip their children, which cut off the menu's rounded corners.
        let root = &self.0.root;
        let (x, y) = view
            .compute_point(root, &gtk::graphene::Point::new(x as f32, y as f32))
            .map(|p| (p.x() as f64, p.y() as f64))
            .unwrap_or((x, y));
        let pop = gtk::PopoverMenu::from_model(Some(&menu));
        pop.set_parent(root);
        pop.set_has_arrow(false);
        pop.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        pop.connect_closed(|p| {
            let p = p.clone();
            glib::idle_add_local_once(move || p.unparent());
        });
        pop.popup();
    }

    /// Stop background work (tab closing).
    pub fn shutdown(&self) {
        if let Some(c) = self.0.scan_cancel.borrow_mut().take() {
            c.cancel();
        }
        if let Some(c) = self.0.dims_cancel.borrow_mut().take() {
            c.cancel();
        }
        if let Some(m) = self.0.monitor.borrow_mut().take() {
            m.cancel();
        }
        self.0.viewer.close();
        self.0.model.clear();
        for id in self.0.settings_handlers.borrow_mut().drain(..) {
            settings().disconnect(id);
        }
    }
}

fn find_thumb_cell(w: &gtk::Widget) -> Option<ThumbCell> {
    if let Some(c) = w.downcast_ref::<ThumbCell>() {
        return Some(c.clone());
    }
    let mut child = w.first_child();
    while let Some(c) = child {
        if let Some(t) = find_thumb_cell(&c) {
            return Some(t);
        }
        child = c.next_sibling();
    }
    None
}

/// Is keyboard focus inside a text entry of `w`'s window? Deferred focus
/// changes must not steal focus from entries the user just opened.
pub fn text_entry_has_focus(w: &impl IsA<gtk::Widget>) -> bool {
    w.root().and_then(|r| r.focus()).is_some_and(|f| {
        f.is::<gtk::Text>()
            || f.is::<gtk::Entry>()
            || f.ancestor(gtk::Text::static_type()).is_some()
            || f.ancestor(gtk::Entry::static_type()).is_some()
    })
}

/// Strip trailing slashes and `.` components; keep symlinks as typed.
pub fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    if out.as_os_str().is_empty() {
        out.push("/");
    }
    out
}

/// A small dialog asking for a name.
pub fn name_dialog(
    parent: &gtk::Window,
    heading: &str,
    accept: &str,
    initial: &str,
    select_stem: Option<usize>,
    done: impl Fn(String) + 'static,
) {
    let dialog = adw::AlertDialog::new(Some(heading), None);
    let entry = gtk::Entry::new();
    entry.set_text(initial);
    entry.set_activates_default(true);
    dialog.set_extra_child(Some(&entry));
    dialog.add_response("cancel", &tr("Cancel"));
    dialog.add_response("ok", accept);
    dialog.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("ok"));
    dialog.set_close_response("cancel");
    let d = dialog.downgrade();
    entry.connect_changed(move |e| {
        if let Some(d) = d.upgrade() {
            let t = e.text();
            d.set_response_enabled("ok", !t.trim().is_empty() && !t.contains('/'));
        }
    });
    let e = entry.clone();
    dialog.connect_response(None, move |_, r| {
        if r == "ok" {
            done(e.text().trim().to_string());
        }
    });
    dialog.present(Some(parent));
    entry.grab_focus();
    let end = select_stem.map(|n| n as i32).unwrap_or(-1);
    entry.select_region(0, end);
}

/// Rename `path` with a dialog; `renamed(old, new)` is called on success.
pub fn rename_dialog(parent: &gtk::Window, path: &Path, renamed: impl Fn(&Path, &Path) + 'static) {
    let name = crate::util::file_name(path);
    let stem_len =
        if path.is_dir() { None } else { Path::new(&name).file_stem().map(|s| s.to_string_lossy().chars().count()) };
    let path = path.to_path_buf();
    let renamed = Rc::new(renamed);
    let window = parent.clone();
    name_dialog(parent, &tr("Rename"), &tr("Rename"), &name, stem_len, move |new_name| {
        let path = path.clone();
        let renamed = renamed.clone();
        let window = window.clone();
        glib::spawn_future_local(async move {
            match ops::rename(&path, &new_name).await {
                Ok(new) => renamed(&path, &new),
                Err(e) => {
                    let d = adw::AlertDialog::new(Some(&tr("Could Not Rename")), Some(e.message()));
                    d.add_response("ok", &tr("OK"));
                    d.present(Some(&window));
                }
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gestures() {
        assert_eq!(classify_gesture(10.0, 5.0), None);
        assert_eq!(classify_gesture(0.0, -80.0), Some(Gesture::Up));
        assert_eq!(classify_gesture(0.0, 80.0), Some(Gesture::Down));
        assert_eq!(classify_gesture(-80.0, 5.0), Some(Gesture::Left));
        assert_eq!(classify_gesture(80.0, -5.0), Some(Gesture::Right));
        assert_eq!(classify_gesture(60.0, -60.0), Some(Gesture::UpRight));
        assert_eq!(classify_gesture(60.0, 60.0), Some(Gesture::DownRight));
        assert_eq!(classify_gesture(-60.0, -70.0), Some(Gesture::Up));
    }

    #[test]
    fn normalizing() {
        assert_eq!(normalize(Path::new("/a/b/../c/./")), PathBuf::from("/a/c"));
        assert_eq!(normalize(Path::new("/..")), PathBuf::from("/"));
    }
}
