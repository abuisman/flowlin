//! A node of the sidebar tree: a section header or a folder whose children
//! are enumerated lazily on a worker thread.

use std::cell::{Cell, OnceCell, RefCell};
use std::path::{Path, PathBuf};

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NodeKind {
    #[default]
    Folder,
    /// A top-level place (Home, Computer, a volume, a favourite).
    Place,
    Header,
}

mod imp {
    use super::*;
    use glib::subclass::Signal;
    use std::sync::OnceLock;

    #[derive(Default)]
    pub struct FolderNode {
        pub path: RefCell<PathBuf>,
        pub label: RefCell<String>,
        pub icon: RefCell<String>,
        pub kind: Cell<NodeKind>,
        pub show_hidden: Cell<bool>,
        pub children: OnceCell<gio::ListStore>,
        pub loading: Cell<bool>,
        pub loaded: Cell<bool>,
        pub no_images: Cell<bool>,
        pub waiters: RefCell<Vec<async_channel::Sender<()>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for FolderNode {
        const NAME: &'static str = "FlowlinFolderNode";
        type Type = super::FolderNode;
    }

    impl ObjectImpl for FolderNode {
        fn signals() -> &'static [Signal] {
            static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| vec![Signal::builder("changed").build()])
        }
    }
}

glib::wrapper! {
    pub struct FolderNode(ObjectSubclass<imp::FolderNode>);
}

impl FolderNode {
    pub fn new(kind: NodeKind, path: &Path, label: &str, icon: &str, show_hidden: bool) -> Self {
        let n: Self = glib::Object::new();
        let imp = n.imp();
        imp.kind.set(kind);
        *imp.path.borrow_mut() = path.to_path_buf();
        *imp.label.borrow_mut() = label.to_string();
        *imp.icon.borrow_mut() = icon.to_string();
        imp.show_hidden.set(show_hidden);
        n
    }

    pub fn header(label: &str) -> Self {
        Self::new(NodeKind::Header, Path::new(""), label, "", false)
    }

    pub fn folder(path: &Path, show_hidden: bool) -> Self {
        Self::new(NodeKind::Folder, path, &crate::util::file_name(path), "folder-symbolic", show_hidden)
    }

    pub fn kind(&self) -> NodeKind {
        self.imp().kind.get()
    }
    pub fn is_header(&self) -> bool {
        self.kind() == NodeKind::Header
    }
    pub fn path(&self) -> PathBuf {
        self.imp().path.borrow().clone()
    }
    pub fn label(&self) -> String {
        self.imp().label.borrow().clone()
    }
    pub fn icon(&self) -> String {
        self.imp().icon.borrow().clone()
    }
    pub fn is_loading(&self) -> bool {
        self.imp().loading.get()
    }
    pub fn no_images(&self) -> bool {
        self.imp().no_images.get()
    }
    pub fn set_no_images(&self, v: bool) {
        if self.imp().no_images.replace(v) != v {
            self.emit_by_name::<()>("changed", &[]);
        }
    }

    pub fn connect_changed<F: Fn(&Self) + 'static>(&self, f: F) -> glib::SignalHandlerId {
        self.connect_closure("changed", false, glib::closure_local!(move |n: &FolderNode| f(n)))
    }

    /// Child model; enumeration starts on first access.
    pub fn children(&self) -> gio::ListStore {
        let imp = self.imp();
        let store = imp.children.get_or_init(gio::ListStore::new::<FolderNode>).clone();
        if !imp.loaded.get() && !imp.loading.get() {
            self.load();
        }
        store
    }

    /// Re-enumerate children (after a folder was created/renamed/removed).
    pub fn reload(&self) {
        if self.imp().children.get().is_some() && !self.imp().loading.get() {
            self.load();
        }
    }

    fn load(&self) {
        let imp = self.imp();
        imp.loading.set(true);
        self.emit_by_name::<()>("changed", &[]);
        let path = self.path();
        let show_hidden = imp.show_hidden.get();
        let (tx, rx) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let dirs = crate::fs::navigate::subdirs(&path, show_hidden);
            let _ = tx.send_blocking(dirs);
        });
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let Ok(dirs) = rx.recv().await else { return };
            let Some(node) = weak.upgrade() else { return };
            node.populate(dirs);
        });
    }

    fn populate(&self, dirs: Vec<PathBuf>) {
        let imp = self.imp();
        let show_hidden = imp.show_hidden.get();
        let nodes: Vec<FolderNode> = dirs.iter().map(|d| FolderNode::folder(d, show_hidden)).collect();
        let store = imp.children.get().unwrap();
        store.splice(0, store.n_items(), &nodes);
        imp.loading.set(false);
        imp.loaded.set(true);
        self.emit_by_name::<()>("changed", &[]);
        for w in imp.waiters.borrow_mut().drain(..) {
            let _ = w.try_send(());
        }
        // Lazily mark folders that contain no images.
        let (tx, rx) = async_channel::unbounded::<(usize, bool)>();
        std::thread::spawn(move || {
            for (i, d) in dirs.iter().enumerate() {
                if tx.send_blocking((i, crate::fs::navigate::has_images(d, show_hidden))).is_err() {
                    break;
                }
            }
        });
        glib::spawn_future_local(async move {
            while let Ok((i, has)) = rx.recv().await {
                if let Some(n) = nodes.get(i) {
                    n.set_no_images(!has);
                }
            }
        });
    }

    /// Resolves once the children have been enumerated.
    pub async fn wait_loaded(&self) {
        let _ = self.children();
        if self.imp().loaded.get() {
            return;
        }
        let (tx, rx) = async_channel::bounded(1);
        self.imp().waiters.borrow_mut().push(tx);
        let _ = rx.recv().await;
    }
}
