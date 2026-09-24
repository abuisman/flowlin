//! Thumbnail views: grid (GtkGridView), list (GtkColumnView) and waterfall
//! (a custom virtualised widget). All render the same tab model.

pub mod cell;
pub mod grid;
pub mod list;
pub mod waterfall;

use std::cell::{Cell, RefCell};

use gtk::glib;

pub use cell::ThumbCell;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewMode {
    #[default]
    Grid,
    Waterfall,
    List,
}

impl ViewMode {
    pub fn id(self) -> &'static str {
        match self {
            ViewMode::Grid => "grid",
            ViewMode::Waterfall => "waterfall",
            ViewMode::List => "list",
        }
    }
    pub fn from_id(s: &str) -> Self {
        match s {
            "waterfall" => ViewMode::Waterfall,
            "list" => ViewMode::List,
            _ => ViewMode::Grid,
        }
    }
}

/// Presentation state shared by all cells of a tab.
pub struct ViewConfig {
    pub size: Cell<i32>,
    pub scale: Cell<i32>,
    pub show_names: Cell<bool>,
    pub show_folders: Cell<bool>,
    cells: RefCell<Vec<glib::WeakRef<ThumbCell>>>,
}

impl Default for ViewConfig {
    fn default() -> Self {
        Self {
            size: Cell::new(crate::config::THUMB_SIZES[2]),
            scale: Cell::new(1),
            show_names: Cell::new(true),
            show_folders: Cell::new(false),
            cells: Default::default(),
        }
    }
}

impl ViewConfig {
    pub fn register(&self, cell: &ThumbCell) {
        let mut cells = self.cells.borrow_mut();
        // Prune dead entries only occasionally: amortised O(1).
        if cells.len() >= 64 && cells.len().is_power_of_two() {
            cells.retain(|w| w.upgrade().is_some());
        }
        let w = glib::WeakRef::new();
        w.set(Some(cell));
        cells.push(w);
    }

    /// Re-apply size / label settings to every live grid cell.
    pub fn refresh_cells(&self) {
        for w in self.cells.borrow().iter() {
            if let Some(c) = w.upgrade() {
                c.apply_config(self);
            }
        }
    }

    /// Device pixels to request thumbnails at.
    pub fn thumb_px(&self) -> i32 {
        self.size.get() * self.scale.get().max(1)
    }
}
