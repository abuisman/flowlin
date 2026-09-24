//! Folder tree sidebar: favourites, places (Home, Pictures, Computer) and
//! mounted volumes, expanded lazily.

mod node;

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};

use gtk::gdk;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

pub use node::{FolderNode, NodeKind};

use crate::i18n::tr;

/// Actions offered by a folder's context menu, handled by the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderAction {
    Open,
    /// Open the folder and focus its first image (Space).
    View,
    OpenRecursive,
    OpenInNewTab,
    OpenInFileManager,
    ToggleFavourite,
    Rename,
    NewFolder,
    Trash,
}

impl FolderAction {
    const ALL: [(FolderAction, &'static str); 8] = [
        (FolderAction::Open, "open"),
        (FolderAction::OpenRecursive, "open-recursive"),
        (FolderAction::OpenInNewTab, "open-tab"),
        (FolderAction::OpenInFileManager, "open-fm"),
        (FolderAction::ToggleFavourite, "favourite"),
        (FolderAction::Rename, "rename"),
        (FolderAction::NewFolder, "new-folder"),
        (FolderAction::Trash, "trash"),
    ];
}

type ActionHandler = Box<dyn Fn(FolderAction, PathBuf)>;
type OpenHandler = Box<dyn Fn(PathBuf, bool)>;

mod row_imp {
    use super::*;

    #[derive(Default)]
    pub struct Row {
        pub expander: gtk::TreeExpander,
        pub icon: gtk::Image,
        pub label: gtk::Label,
        pub spinner: gtk::Spinner,
        pub bound: RefCell<Option<(FolderNode, glib::SignalHandlerId)>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Row {
        const NAME: &'static str = "FlowlinSidebarRow";
        type Type = super::Row;
        type ParentType = gtk::Box;
    }

    impl ObjectImpl for Row {
        fn constructed(&self) {
            self.parent_constructed();
            let content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            content.append(&self.icon);
            self.label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            self.label.set_xalign(0.0);
            self.label.set_hexpand(true);
            content.append(&self.label);
            content.append(&self.spinner);
            self.expander.set_child(Some(&content));
            self.expander.set_hexpand(true);
            self.obj().append(&self.expander);
        }
    }
    impl WidgetImpl for Row {}
    impl BoxImpl for Row {}
}

glib::wrapper! {
    pub struct Row(ObjectSubclass<row_imp::Row>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl Row {
    /// Folder shown by this row (None for section headers).
    pub fn folder_path(&self) -> Option<PathBuf> {
        self.node().filter(|n| !n.is_header()).map(|n| n.path())
    }

    fn node(&self) -> Option<FolderNode> {
        self.imp().bound.borrow().as_ref().map(|(n, _)| n.clone())
    }

    fn bind(&self, row: &gtk::TreeListRow) {
        self.unbind();
        let imp = self.imp();
        imp.expander.set_list_row(Some(row));
        let Some(node) = row.item().and_downcast::<FolderNode>() else { return };
        let w = self.downgrade();
        let id = node.connect_changed(move |_| {
            if let Some(r) = w.upgrade() {
                r.render();
            }
        });
        *imp.bound.borrow_mut() = Some((node, id));
        self.render();
    }

    fn unbind(&self) {
        if let Some((n, id)) = self.imp().bound.borrow_mut().take() {
            n.disconnect(id);
        }
        self.imp().expander.set_list_row(None);
    }

    fn render(&self) {
        let imp = self.imp();
        let Some(node) = self.node() else { return };
        imp.label.set_label(&node.label());
        let header = node.is_header();
        imp.icon.set_visible(!header);
        if !header {
            imp.icon.set_icon_name(Some(&node.icon()));
        }
        imp.expander.set_hide_expander(header);
        imp.expander.set_indent_for_icon(!header);
        if header {
            imp.label.add_css_class("sidebar-heading");
            imp.label.add_css_class("dim-label");
        } else {
            imp.label.remove_css_class("sidebar-heading");
            if node.no_images() {
                imp.label.add_css_class("dim-label");
            } else {
                imp.label.remove_css_class("dim-label");
            }
        }
        let loading = node.is_loading();
        imp.spinner.set_visible(loading);
        imp.spinner.set_spinning(loading);
        let path = node.path();
        self.set_tooltip_text(if header { None } else { path.to_str() });
    }
}

pub struct Inner {
    pub widget: gtk::ScrolledWindow,
    roots: gio::ListStore,
    tree: gtk::TreeListModel,
    selection: gtk::SingleSelection,
    view: gtk::ListView,
    suppress: Cell<bool>,
    reveal_gen: Cell<u64>,
    current: RefCell<Option<PathBuf>>,
    on_open: RefCell<Option<OpenHandler>>,
    on_action: RefCell<Option<ActionHandler>>,
    pending_open: RefCell<Option<glib::SourceId>>,
    on_leave: RefCell<Option<Box<dyn Fn()>>>,
    volumes: gio::VolumeMonitor,
}

#[derive(Clone)]
pub struct Sidebar(Rc<Inner>);

impl Sidebar {
    pub fn new() -> Self {
        let roots = gio::ListStore::new::<FolderNode>();
        let tree = gtk::TreeListModel::new(roots.clone(), false, false, |obj| {
            let node = obj.downcast_ref::<FolderNode>()?;
            (!node.is_header()).then(|| node.children().upcast())
        });
        let selection = gtk::SingleSelection::new(Some(tree.clone()));
        selection.set_autoselect(false);
        selection.set_can_unselect(true);

        let factory = gtk::SignalListItemFactory::new();
        let view = gtk::ListView::new(Some(selection.clone()), Some(factory.clone()));
        view.add_css_class("navigation-sidebar");
        let widget = gtk::ScrolledWindow::new();
        widget.set_hscrollbar_policy(gtk::PolicyType::Never);
        widget.set_child(Some(&view));
        widget.set_vexpand(true);

        let s = Sidebar(Rc::new(Inner {
            widget,
            roots,
            tree,
            selection,
            view,
            suppress: Cell::new(false),
            reveal_gen: Cell::new(0),
            current: Default::default(),
            on_open: Default::default(),
            on_action: Default::default(),
            pending_open: Default::default(),
            on_leave: Default::default(),
            volumes: gio::VolumeMonitor::get(),
        }));

        let w = s.downgrade();
        factory.connect_setup(move |_, obj| {
            let li = obj.downcast_ref::<gtk::ListItem>().unwrap();
            let row: Row = glib::Object::new();
            if let Some(s) = upgrade(&w) {
                s.setup_row(&row);
            }
            li.set_child(Some(&row));
        });
        factory.connect_bind(|_, obj| {
            let li = obj.downcast_ref::<gtk::ListItem>().unwrap();
            let row = li.child().and_downcast::<Row>().unwrap();
            let tr = li.item().and_downcast::<gtk::TreeListRow>().unwrap();
            let header = tr.item().and_downcast::<FolderNode>().is_some_and(|n| n.is_header());
            li.set_selectable(!header);
            li.set_activatable(!header);
            li.set_focusable(!header);
            row.bind(&tr);
        });
        factory.connect_unbind(|_, obj| {
            let li = obj.downcast_ref::<gtk::ListItem>().unwrap();
            if let Some(row) = li.child().and_downcast::<Row>() {
                row.unbind();
            }
        });

        let w = s.downgrade();
        s.0.view.connect_activate(move |_, pos| {
            if let Some(s) = upgrade(&w) {
                if let Some(node) = s.node_at(pos) {
                    s.emit_open(node.path(), false);
                }
            }
        });
        s.setup_keys();

        let w = s.downgrade();
        crate::settings::settings().connect_changed(Some("favourites"), move |_, _| {
            if let Some(s) = upgrade(&w) {
                s.rebuild();
            }
        });
        let w = s.downgrade();
        crate::settings::settings().connect_changed(Some("show-hidden"), move |_, _| {
            if let Some(s) = upgrade(&w) {
                s.rebuild();
            }
        });
        for sig in ["mount-added", "mount-removed", "mount-changed"] {
            let w = s.downgrade();
            s.0.volumes.connect_local(sig, false, move |_| {
                if let Some(s) = upgrade(&w) {
                    s.rebuild();
                }
                None
            });
        }
        s.rebuild();
        s
    }

    fn downgrade(&self) -> Weak<Inner> {
        Rc::downgrade(&self.0)
    }

    pub fn widget(&self) -> &gtk::ScrolledWindow {
        &self.0.widget
    }

    pub fn connect_open(&self, f: impl Fn(PathBuf, bool) + 'static) {
        *self.0.on_open.borrow_mut() = Some(Box::new(f));
    }

    /// Tab pressed in the tree: the window moves focus to the thumbnails.
    pub fn connect_leave(&self, f: impl Fn() + 'static) {
        *self.0.on_leave.borrow_mut() = Some(Box::new(f));
    }

    /// Give keyboard focus to the selected folder row.
    pub fn focus_selected(&self) {
        let sel = self.0.selection.selected();
        // scroll_to(FOCUS) only moves focus within an already focused list.
        self.0.view.grab_focus();
        if sel != gtk::INVALID_LIST_POSITION {
            self.0.view.scroll_to(sel, gtk::ListScrollFlags::FOCUS, None);
        }
    }

    pub fn connect_action(&self, f: impl Fn(FolderAction, PathBuf) + 'static) {
        *self.0.on_action.borrow_mut() = Some(Box::new(f));
    }

    fn emit_open(&self, path: PathBuf, new_tab: bool) {
        if let Some(f) = self.0.on_open.borrow().as_ref() {
            f(path, new_tab);
        }
    }

    fn emit_action(&self, a: FolderAction, path: PathBuf) {
        if let Some(f) = self.0.on_action.borrow().as_ref() {
            f(a, path);
        }
    }

    fn node_at(&self, pos: u32) -> Option<FolderNode> {
        self.0
            .tree
            .item(pos)
            .and_downcast::<gtk::TreeListRow>()?
            .item()
            .and_downcast::<FolderNode>()
            .filter(|n| !n.is_header())
    }

    /// Rebuild the top level (favourites, places, devices).
    pub fn rebuild(&self) {
        let show_hidden = crate::settings::settings().boolean("show-hidden");
        let mut nodes = Vec::new();
        let favs = crate::settings::favourites();
        if !favs.is_empty() {
            nodes.push(FolderNode::header(&tr("Favourites")));
            for p in favs {
                nodes.push(FolderNode::new(
                    NodeKind::Place,
                    &p,
                    &crate::util::file_name(&p),
                    "starred-symbolic",
                    show_hidden,
                ));
            }
        }
        nodes.push(FolderNode::header(&tr("Places")));
        let home = glib::home_dir();
        nodes.push(FolderNode::new(NodeKind::Place, &home, &tr("Home"), "user-home-symbolic", show_hidden));
        if let Some(pics) = glib::user_special_dir(glib::UserDirectory::Pictures) {
            if pics != home && pics.is_dir() {
                nodes.push(FolderNode::new(
                    NodeKind::Place,
                    &pics,
                    &tr("Pictures"),
                    "folder-pictures-symbolic",
                    show_hidden,
                ));
            }
        }
        nodes.push(FolderNode::new(
            NodeKind::Place,
            Path::new("/"),
            &tr("Computer"),
            "drive-harddisk-symbolic",
            show_hidden,
        ));
        let mounts: Vec<_> = self
            .0
            .volumes
            .mounts()
            .into_iter()
            .filter(|m| !m.is_shadowed())
            .filter_map(|m| {
                let path = m.root().path()?;
                let icon = m
                    .symbolic_icon()
                    .downcast::<gio::ThemedIcon>()
                    .ok()
                    .and_then(|i| i.names().first().map(|s| s.to_string()))
                    .unwrap_or_else(|| "drive-removable-media-symbolic".into());
                Some(FolderNode::new(NodeKind::Place, &path, &m.name(), &icon, show_hidden))
            })
            .collect();
        if !mounts.is_empty() {
            nodes.push(FolderNode::header(&tr("Devices")));
            nodes.extend(mounts);
        }
        self.0.roots.splice(0, self.0.roots.n_items(), &nodes);
        let current = self.0.current.borrow().clone();
        if let Some(p) = current {
            self.reveal(&p);
        }
    }

    /// Re-enumerate the children of `dir` wherever it appears in the tree.
    pub fn refresh(&self, dir: &Path) {
        for i in 0..self.0.tree.n_items() {
            if let Some(n) = self
                .0
                .tree
                .item(i)
                .and_downcast::<gtk::TreeListRow>()
                .and_then(|r| r.item().and_downcast::<FolderNode>())
            {
                if !n.is_header() && n.path() == dir {
                    n.reload();
                }
            }
        }
    }

    /// Highlight `path`, expanding its ancestors (async).
    pub fn reveal(&self, path: &Path) {
        *self.0.current.borrow_mut() = Some(path.to_path_buf());
        let gen = self.0.reveal_gen.get() + 1;
        self.0.reveal_gen.set(gen);
        let w = self.downgrade();
        let path = path.to_path_buf();
        glib::spawn_future_local(async move {
            let Some(s) = upgrade(&w) else { return };
            s.reveal_async(path, gen).await;
        });
    }

    async fn reveal_async(&self, path: PathBuf, gen: u64) {
        // Pick the top-level place with the longest matching prefix.
        let mut best: Option<(usize, u32)> = None;
        for i in 0..self.0.roots.n_items() {
            let Some(n) = self.0.roots.item(i).and_downcast::<FolderNode>() else { continue };
            if n.is_header() {
                continue;
            }
            let p = n.path();
            if path.starts_with(&p) {
                let len = p.components().count();
                if best.is_none_or(|(l, _)| len > l) {
                    best = Some((len, i));
                }
            }
        }
        let Some((_, root_idx)) = best else { return };
        // Top-level rows: find the tree row of root_idx.
        let mut row = None;
        for i in 0..self.0.tree.n_items() {
            if let Some(r) = self.0.tree.item(i).and_downcast::<gtk::TreeListRow>() {
                if r.depth() == 0 && r.item().as_ref() == self.0.roots.item(root_idx).as_ref() {
                    row = Some(r);
                    break;
                }
            }
        }
        let Some(mut row) = row else { return };
        loop {
            if self.0.reveal_gen.get() != gen {
                return;
            }
            let Some(node) = row.item().and_downcast::<FolderNode>() else { return };
            if node.path() == path {
                self.select_row(&row);
                return;
            }
            node.wait_loaded().await;
            if self.0.reveal_gen.get() != gen {
                return;
            }
            row.set_expanded(true);
            let children = node.children();
            let next = (0..children.n_items())
                .find(|&i| children.item(i).and_downcast::<FolderNode>().is_some_and(|c| path.starts_with(c.path())));
            match next.and_then(|i| row.child_row(i)) {
                Some(r) => row = r,
                None => {
                    // Hidden or vanished folder: highlight the nearest ancestor.
                    self.select_row(&row);
                    return;
                }
            }
        }
    }

    fn select_row(&self, row: &gtk::TreeListRow) {
        let pos = row.position();
        self.0.suppress.set(true);
        self.0.selection.set_selected(pos);
        self.0.suppress.set(false);
        // Scroll once the freshly expanded rows have been laid out.
        let view = self.0.view.clone();
        glib::idle_add_local_once(move || view.scroll_to(pos, gtk::ListScrollFlags::NONE, None));
    }

    fn setup_row(&self, row: &Row) {
        // Left click navigates; middle click opens a new tab.
        let click = gtk::GestureClick::new();
        click.set_button(0);
        let w = self.downgrade();
        let r = row.downgrade();
        click.connect_released(move |g, n, x, y| {
            let (Some(s), Some(row)) = (upgrade(&w), r.upgrade()) else { return };
            let Some(node) = row.node().filter(|n| !n.is_header()) else { return };
            // Ignore clicks on the disclosure triangle.
            if let Some(picked) = row.pick(x, y, gtk::PickFlags::DEFAULT) {
                if picked.css_name() == "expander" || picked.has_css_class("expander") {
                    return;
                }
            }
            match g.current_button() {
                gdk::BUTTON_PRIMARY if n == 1 => s.emit_open(node.path(), false),
                gdk::BUTTON_MIDDLE => s.emit_open(node.path(), true),
                _ => {}
            }
        });
        row.add_controller(click);

        let menu_click = gtk::GestureClick::new();
        menu_click.set_button(gdk::BUTTON_SECONDARY);
        let w = self.downgrade();
        let r = row.downgrade();
        menu_click.connect_pressed(move |_, _, x, y| {
            let (Some(s), Some(row)) = (upgrade(&w), r.upgrade()) else { return };
            let Some(node) = row.node().filter(|n| !n.is_header()) else { return };
            s.popup_menu(&row, node.path(), x, y);
        });
        row.add_controller(menu_click);

        let group = gio::SimpleActionGroup::new();
        for (act, name) in FolderAction::ALL {
            let a = gio::SimpleAction::new(name, None);
            let w = self.downgrade();
            let r = row.downgrade();
            a.connect_activate(move |_, _| {
                let (Some(s), Some(row)) = (upgrade(&w), r.upgrade()) else { return };
                if let Some(node) = row.node() {
                    s.emit_action(act, node.path());
                }
            });
            group.add_action(&a);
        }
        row.insert_action_group("folder", Some(&group));
    }

    fn popup_menu(&self, row: &Row, path: PathBuf, x: f64, y: f64) {
        let menu = gio::Menu::new();
        let s1 = gio::Menu::new();
        s1.append(Some(&tr("Open")), Some("folder.open"));
        s1.append(Some(&tr("Open with Subfolders")), Some("folder.open-recursive"));
        s1.append(Some(&tr("Open in New Tab")), Some("folder.open-tab"));
        s1.append(Some(&tr("Open in File Manager")), Some("folder.open-fm"));
        menu.append_section(None, &s1);
        let s2 = gio::Menu::new();
        let fav =
            if crate::settings::is_favourite(&path) { tr("Remove from Favourites") } else { tr("Add to Favourites") };
        s2.append(Some(&fav), Some("folder.favourite"));
        menu.append_section(None, &s2);
        let s3 = gio::Menu::new();
        s3.append(Some(&tr("New Folder…")), Some("folder.new-folder"));
        s3.append(Some(&tr("Rename…")), Some("folder.rename"));
        s3.append(Some(&tr("Move to Trash")), Some("folder.trash"));
        menu.append_section(None, &s3);
        let pop = gtk::PopoverMenu::from_model(Some(&menu));
        pop.set_parent(row);
        pop.set_has_arrow(false);
        pop.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        pop.connect_closed(|p| {
            let p = p.clone();
            glib::idle_add_local_once(move || p.unparent());
        });
        pop.popup();
    }

    /// Move the selection to the next folder row above/below (skipping
    /// section headers), like the arrow keys do.
    fn move_selection(&self, delta: i32) {
        let n = self.0.tree.n_items() as i64;
        let mut pos = self.0.selection.selected() as i64;
        if self.0.selection.selected() == gtk::INVALID_LIST_POSITION {
            pos = if delta > 0 { -1 } else { n };
        }
        loop {
            pos += delta as i64;
            if pos < 0 || pos >= n {
                return;
            }
            if self.node_at(pos as u32).is_some() {
                break;
            }
        }
        self.0.selection.set_selected(pos as u32);
        let flags = if self.0.view.has_focus() || self.0.view.focus_child().is_some() {
            gtk::ListScrollFlags::FOCUS
        } else {
            gtk::ListScrollFlags::NONE
        };
        self.0.view.scroll_to(pos as u32, flags, None);
    }

    /// Arrow keys (and W/A/S/D) on the tree: Up/Down move to the previous /
    /// next folder row, Right expands, Left collapses or goes to the parent.
    /// Moving to another folder opens it in the current tab (after a short
    /// pause, so holding a key does not start a scan for every row).
    pub fn tree_key(&self, key: gdk::Key) -> bool {
        use gdk::Key;
        let key = match key {
            Key::w | Key::W => Key::Up,
            Key::a | Key::A => Key::Left,
            Key::s | Key::S => Key::Down,
            Key::d | Key::D => Key::Right,
            k => k,
        };
        let before = self.0.selection.selected();
        match key {
            Key::Up | Key::KP_Up => self.move_selection(-1),
            Key::Down | Key::KP_Down => self.move_selection(1),
            Key::Right | Key::KP_Right | Key::Left | Key::KP_Left => {
                let Some(row) = self.0.tree.item(before).and_downcast::<gtk::TreeListRow>() else { return true };
                if matches!(key, Key::Right | Key::KP_Right) {
                    row.set_expanded(true);
                } else if row.is_expanded() {
                    row.set_expanded(false);
                } else if let Some(parent) = row.parent() {
                    let pos = parent.position();
                    self.0.selection.set_selected(pos);
                    self.0.view.scroll_to(pos, gtk::ListScrollFlags::NONE, None);
                }
            }
            _ => return false,
        }
        let after = self.0.selection.selected();
        if after != before {
            if let Some(node) = self.node_at(after) {
                self.open_soon(node.path());
            }
        }
        true
    }

    fn open_soon(&self, path: PathBuf) {
        if let Some(id) = self.0.pending_open.borrow_mut().take() {
            id.remove();
        }
        let w = self.downgrade();
        let id = glib::timeout_add_local_once(std::time::Duration::from_millis(150), move || {
            if let Some(s) = upgrade(&w) {
                s.0.pending_open.borrow_mut().take();
                s.emit_open(path, false);
            }
        });
        *self.0.pending_open.borrow_mut() = Some(id);
    }

    fn setup_keys(&self) {
        let keys = gtk::EventControllerKey::new();
        // Capture phase: rows would otherwise consume Space themselves.
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let w = self.downgrade();
        keys.connect_key_pressed(move |_, key, _, state| {
            use gdk::Key;
            let Some(s) = upgrade(&w) else { return glib::Propagation::Proceed };
            if state.intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK) {
                return glib::Propagation::Proceed;
            }
            if matches!(key, Key::Tab | Key::ISO_Left_Tab | Key::KP_Tab) {
                if let Some(f) = s.0.on_leave.borrow().as_ref() {
                    f();
                }
                return glib::Propagation::Stop;
            }
            if s.tree_key(key) {
                return glib::Propagation::Stop;
            }
            let sel = s.0.selection.selected();
            let Some(row) = s.0.tree.item(sel).and_downcast::<gtk::TreeListRow>() else {
                return glib::Propagation::Proceed;
            };
            match key {
                Key::space => {
                    if let Some(n) = row.item().and_downcast::<FolderNode>().filter(|n| !n.is_header()) {
                        s.emit_action(FolderAction::View, n.path());
                    }
                }
                Key::F2 => {
                    if let Some(n) = row.item().and_downcast::<FolderNode>() {
                        s.emit_action(FolderAction::Rename, n.path());
                    }
                }
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
        self.0.view.add_controller(keys);
    }

    pub fn focus(&self) {
        self.0.view.grab_focus();
    }
}

impl Default for Sidebar {
    fn default() -> Self {
        Self::new()
    }
}

fn upgrade(w: &Weak<Inner>) -> Option<Sidebar> {
    w.upgrade().map(Sidebar)
}
