use std::rc::Rc;

use gtk::prelude::*;

use super::{ThumbCell, ViewConfig};
use crate::model::{BrowserModel, ImageItem};

/// Set the grid's column limit to what fits `width` at the current cell size.
pub fn fit_columns(grid: &gtk::GridView, width: f64, cfg: &ViewConfig) {
    // Cell = card + 2×8 px padding + 2×2 px margin (style.css).
    let cell = (cfg.size.get() + 20) as f64;
    let cols = ((width - 12.0) / cell).floor().clamp(1.0, 64.0) as u32;
    if grid.max_columns() != cols {
        grid.set_max_columns(cols);
    }
}

pub fn create(model: &BrowserModel, cfg: Rc<ViewConfig>) -> gtk::GridView {
    let factory = gtk::SignalListItemFactory::new();
    let c = cfg.clone();
    factory.connect_setup(move |_, obj| {
        let li = obj.downcast_ref::<gtk::ListItem>().unwrap();
        let cell = ThumbCell::new(false);
        cell.apply_size(&c);
        c.register(&cell);
        li.set_child(Some(&cell));
    });
    let c = cfg.clone();
    factory.connect_bind(move |_, obj| {
        let li = obj.downcast_ref::<gtk::ListItem>().unwrap();
        let cell = li.child().and_downcast::<ThumbCell>().unwrap();
        let item = li.item().and_downcast::<ImageItem>().unwrap();
        cell.bind(&item, &c);
    });
    factory.connect_unbind(move |_, obj| {
        let li = obj.downcast_ref::<gtk::ListItem>().unwrap();
        if let Some(cell) = li.child().and_downcast::<ThumbCell>() {
            cell.unbind();
        }
    });
    let grid = gtk::GridView::new(Some(model.selection.clone()), Some(factory));
    // GtkGridView keeps about max_columns × 32 cell widgets alive, so keep
    // max_columns at what actually fits (see `fit_columns`).
    grid.set_max_columns(4);
    grid.set_min_columns(1);
    grid.set_enable_rubberband(true);
    grid.add_css_class("flowlin-grid");
    grid
}
