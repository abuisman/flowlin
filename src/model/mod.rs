//! The per-tab image model: a `gio::ListStore` of [`ImageItem`]s, filtered,
//! sorted and wrapped in a `gtk::MultiSelection`. All views of a tab and the
//! viewer share this one model.

mod filter;
mod item;
mod sort;

pub use filter::{parse_filter, FilterSpec};
pub use item::{ImageItem, ThumbState};
pub use sort::{SortKey, SortState};

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

use crate::fs::scan::FileEntry;

#[derive(Clone)]
pub struct BrowserModel {
    pub store: gio::ListStore,
    pub filter_model: gtk::FilterListModel,
    pub sort_model: gtk::SortListModel,
    pub selection: gtk::MultiSelection,
    sorter: gtk::CustomSorter,
    filter: gtk::CustomFilter,
    sort_state: Rc<RefCell<SortState>>,
    filter_spec: Rc<RefCell<FilterSpec>>,
    index: Rc<RefCell<HashMap<PathBuf, ImageItem>>>,
}

impl Default for BrowserModel {
    fn default() -> Self {
        Self::new()
    }
}

impl BrowserModel {
    pub fn new() -> Self {
        let store = gio::ListStore::new::<ImageItem>();
        let sort_state = Rc::new(RefCell::new(SortState::default()));
        let filter_spec = Rc::new(RefCell::new(FilterSpec::default()));

        let fs = filter_spec.clone();
        let filter = gtk::CustomFilter::new(move |o| {
            let item = o.downcast_ref::<ImageItem>().unwrap();
            fs.borrow().matches(item)
        });
        let st = sort_state.clone();
        let sorter = gtk::CustomSorter::new(move |a, b| {
            let a = a.downcast_ref::<ImageItem>().unwrap();
            let b = b.downcast_ref::<ImageItem>().unwrap();
            st.borrow().compare(a, b).into()
        });
        let filter_model = gtk::FilterListModel::new(Some(store.clone()), Some(filter.clone()));
        let sort_model = gtk::SortListModel::new(Some(filter_model.clone()), Some(sorter.clone()));
        // Sort in time slices so 100k-item folders never block a frame.
        sort_model.set_incremental(true);
        let selection = gtk::MultiSelection::new(Some(sort_model.clone()));
        Self {
            store,
            filter_model,
            sort_model,
            selection,
            sorter,
            filter,
            sort_state,
            filter_spec,
            index: Default::default(),
        }
    }

    pub fn clear(&self) {
        self.store.remove_all();
        self.index.borrow_mut().clear();
    }

    pub fn extend(&self, entries: Vec<FileEntry>) {
        let mut index = self.index.borrow_mut();
        let mut items = Vec::with_capacity(entries.len());
        for e in entries {
            if index.contains_key(&e.path) {
                continue;
            }
            let item = ImageItem::new(&e);
            index.insert(e.path, item.clone());
            items.push(item);
        }
        drop(index);
        self.store.extend_from_slice(&items);
    }

    pub fn get(&self, path: &Path) -> Option<ImageItem> {
        self.index.borrow().get(path).cloned()
    }

    pub fn remove(&self, path: &Path) -> Option<ImageItem> {
        let item = self.index.borrow_mut().remove(path)?;
        if let Some(pos) = self.store.find(&item) {
            self.store.remove(pos);
        }
        Some(item)
    }

    /// Re-key an item after a rename.
    pub fn renamed(&self, old: &Path, new: &Path) {
        let item = self.index.borrow_mut().remove(old);
        if let Some(item) = item {
            item.set_path(new);
            self.index.borrow_mut().insert(new.to_path_buf(), item.clone());
            self.item_changed(&item);
        }
    }

    /// Re-sort / re-filter after an item's sort-relevant data changed.
    pub fn item_changed(&self, item: &ImageItem) {
        if let Some(pos) = self.store.find(item) {
            self.store.items_changed(pos, 1, 1);
        }
    }

    /// Unfiltered item count.
    pub fn total(&self) -> u32 {
        self.store.n_items()
    }

    /// Visible (filtered) item count.
    pub fn n_items(&self) -> u32 {
        self.sort_model.n_items()
    }

    pub fn item(&self, pos: u32) -> Option<ImageItem> {
        self.sort_model.item(pos).and_downcast()
    }

    /// Position of `item` in sorted/filtered order. Linear, but only used on
    /// discrete user actions.
    pub fn position_of(&self, item: &ImageItem) -> Option<u32> {
        (0..self.sort_model.n_items()).find(|&i| self.sort_model.item(i).as_ref() == Some(item.upcast_ref()))
    }

    pub fn position_of_path(&self, path: &Path) -> Option<u32> {
        let item = self.get(path)?;
        self.position_of(&item)
    }

    pub fn sort_state(&self) -> SortState {
        self.sort_state.borrow().clone()
    }

    pub fn set_sort(&self, key: SortKey, descending: bool) {
        {
            let mut s = self.sort_state.borrow_mut();
            if s.key == key && s.descending == descending {
                return;
            }
            s.key = key;
            s.descending = descending;
        }
        if key == SortKey::Random {
            for i in 0..self.store.n_items() {
                if let Some(it) = self.store.item(i).and_downcast::<ImageItem>() {
                    it.reshuffle();
                }
            }
        }
        self.sorter.changed(gtk::SorterChange::Different);
    }

    /// Run `f` once the (incremental) sort has placed every item.
    pub fn when_sorted(&self, f: impl FnOnce() + 'static) {
        if self.sort_model.pending() == 0 {
            f();
            return;
        }
        let f = std::cell::RefCell::new(Some(f));
        let id: Rc<std::cell::RefCell<Option<glib::SignalHandlerId>>> = Default::default();
        let id2 = id.clone();
        let handler = self.sort_model.connect_pending_notify(move |m| {
            if m.pending() == 0 {
                if let Some(f) = f.borrow_mut().take() {
                    f();
                }
                if let Some(h) = id2.borrow_mut().take() {
                    m.disconnect(h);
                }
            }
        });
        *id.borrow_mut() = Some(handler);
    }

    /// Sort again after sort-relevant data changed for many items.
    pub fn resort(&self) {
        self.sorter.changed(gtk::SorterChange::Different);
    }

    /// Filter again (e.g. after ratings were loaded).
    pub fn refilter(&self) {
        self.filter.changed(gtk::FilterChange::Different);
    }

    pub fn needs_ratings(&self) -> bool {
        self.filter_spec.borrow().rating.is_some()
    }

    pub fn filter_text(&self) -> String {
        self.filter_spec.borrow().text.clone()
    }

    pub fn set_filter(&self, text: &str) {
        let spec = parse_filter(text);
        if *self.filter_spec.borrow() == spec {
            return;
        }
        *self.filter_spec.borrow_mut() = spec;
        self.filter.changed(gtk::FilterChange::Different);
    }

    pub fn is_filtered(&self) -> bool {
        !self.filter_spec.borrow().is_empty()
    }

    pub fn selected_items(&self) -> Vec<ImageItem> {
        let sel = self.selection.selection();
        let mut out = Vec::new();
        if let Some((iter, first)) = gtk::BitsetIter::init_first(&sel) {
            for pos in std::iter::once(first).chain(iter) {
                if let Some(it) = self.item(pos) {
                    out.push(it);
                }
            }
        }
        out
    }

    pub fn selected_paths(&self) -> Vec<PathBuf> {
        self.selected_items().iter().map(|i| i.path()).collect()
    }

    /// Index of the first selected item.
    pub fn first_selected(&self) -> Option<u32> {
        let sel = self.selection.selection();
        (!sel.is_empty()).then(|| sel.minimum())
    }

    pub fn select_only(&self, pos: u32) {
        if pos < self.n_items() {
            self.selection.select_item(pos, true);
        }
    }

    pub fn all_items(&self) -> Vec<ImageItem> {
        (0..self.store.n_items()).filter_map(|i| self.store.item(i).and_downcast()).collect()
    }
}
