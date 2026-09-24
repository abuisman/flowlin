use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};

use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::fs::scan::FileEntry;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThumbState {
    #[default]
    None,
    Pending,
    Ready,
    Failed,
}

mod imp {
    use super::*;
    use glib::subclass::Signal;
    use std::sync::OnceLock;

    #[derive(Default)]
    pub struct ImageItem {
        pub path: RefCell<PathBuf>,
        pub name: RefCell<String>,
        pub rel_dir: RefCell<String>,
        pub size: Cell<u64>,
        pub mtime: Cell<i64>,
        pub ctime: Cell<i64>,
        pub width: Cell<u32>,
        pub height: Cell<u32>,
        pub rating: Cell<Option<u8>>,
        pub random: Cell<u32>,
        pub texture: RefCell<Option<gdk::Texture>>,
        pub thumb_state: Cell<ThumbState>,
        /// How many cell widgets currently show this item.
        pub bound: Cell<u32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ImageItem {
        const NAME: &'static str = "FlowlinImageItem";
        type Type = super::ImageItem;
    }

    impl ObjectImpl for ImageItem {
        fn signals() -> &'static [Signal] {
            static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
            // "changed": thumbnail, dimensions, name or rating changed.
            SIGNALS.get_or_init(|| vec![Signal::builder("changed").build()])
        }
    }
}

glib::wrapper! {
    pub struct ImageItem(ObjectSubclass<imp::ImageItem>);
}

impl ImageItem {
    pub fn new(e: &FileEntry) -> Self {
        let obj: Self = glib::Object::new();
        let imp = obj.imp();
        *imp.path.borrow_mut() = e.path.clone();
        *imp.name.borrow_mut() = e.name.clone();
        *imp.rel_dir.borrow_mut() = e.rel_dir.clone();
        imp.size.set(e.size);
        imp.mtime.set(e.mtime);
        imp.ctime.set(e.ctime);
        imp.random.set(glib::random_int());
        obj
    }

    pub fn path(&self) -> PathBuf {
        self.imp().path.borrow().clone()
    }
    pub fn with_path<R>(&self, f: impl FnOnce(&Path) -> R) -> R {
        f(&self.imp().path.borrow())
    }
    pub fn set_path(&self, p: &Path) {
        *self.imp().path.borrow_mut() = p.to_path_buf();
        *self.imp().name.borrow_mut() = crate::util::file_name(p);
        self.emit_changed();
    }
    pub fn name(&self) -> String {
        self.imp().name.borrow().clone()
    }
    pub fn with_name<R>(&self, f: impl FnOnce(&str) -> R) -> R {
        f(&self.imp().name.borrow())
    }
    pub fn rel_dir(&self) -> String {
        self.imp().rel_dir.borrow().clone()
    }
    pub fn with_rel_dir<R>(&self, f: impl FnOnce(&str) -> R) -> R {
        f(&self.imp().rel_dir.borrow())
    }
    pub fn size(&self) -> u64 {
        self.imp().size.get()
    }
    pub fn mtime(&self) -> i64 {
        self.imp().mtime.get()
    }
    pub fn ctime(&self) -> i64 {
        self.imp().ctime.get()
    }
    pub fn random(&self) -> u32 {
        self.imp().random.get()
    }
    pub fn reshuffle(&self) {
        self.imp().random.set(glib::random_int());
    }

    /// Refresh size/mtime after an external modification.
    pub fn update_stat(&self, e: &FileEntry) {
        self.imp().size.set(e.size);
        self.imp().mtime.set(e.mtime);
        self.imp().ctime.set(e.ctime);
    }

    /// (width, height) if known.
    pub fn dimensions(&self) -> Option<(u32, u32)> {
        let (w, h) = (self.imp().width.get(), self.imp().height.get());
        (w > 0 && h > 0).then_some((w, h))
    }
    pub fn set_dimensions(&self, w: u32, h: u32) {
        if self.dimensions() != Some((w, h)) {
            self.imp().width.set(w);
            self.imp().height.set(h);
            self.emit_changed();
        }
    }
    /// Height / width, 1.0 when unknown.
    pub fn aspect(&self) -> f64 {
        match self.dimensions() {
            Some((w, h)) => (h as f64 / w as f64).clamp(0.2, 5.0),
            None => 1.0,
        }
    }

    pub fn rating(&self) -> Option<u8> {
        self.imp().rating.get()
    }
    pub fn set_rating(&self, r: Option<u8>) {
        if self.imp().rating.get() != r {
            self.imp().rating.set(r);
            self.emit_changed();
        }
    }

    pub fn texture(&self) -> Option<gdk::Texture> {
        self.imp().texture.borrow().clone()
    }
    pub fn set_texture(&self, t: Option<gdk::Texture>) {
        let changed = self.imp().texture.borrow().as_ref() != t.as_ref();
        *self.imp().texture.borrow_mut() = t;
        if changed {
            self.emit_changed();
        }
    }

    pub fn thumb_state(&self) -> ThumbState {
        self.imp().thumb_state.get()
    }
    pub fn set_thumb_state(&self, s: ThumbState) {
        let old = self.imp().thumb_state.replace(s);
        if old != s && (s == ThumbState::Failed || old == ThumbState::Failed) {
            self.emit_changed();
        }
    }

    pub fn bound(&self) -> u32 {
        self.imp().bound.get()
    }
    pub fn bind_ref(&self) {
        self.imp().bound.set(self.bound() + 1);
    }
    pub fn bind_unref(&self) {
        self.imp().bound.set(self.bound().saturating_sub(1));
    }

    fn emit_changed(&self) {
        self.emit_by_name::<()>("changed", &[]);
    }

    pub fn connect_changed<F: Fn(&Self) + 'static>(&self, f: F) -> glib::SignalHandlerId {
        self.connect_closure("changed", false, glib::closure_local!(move |item: &ImageItem| f(item)))
    }
}
