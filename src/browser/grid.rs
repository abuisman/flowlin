use std::rc::Rc;

use gtk::prelude::*;

use super::{ThumbCell, ViewConfig};
use crate::model::{BrowserModel, ImageItem};

pub fn create(model: &BrowserModel, cfg: Rc<ViewConfig>) -> gtk::GridView {
    let factory = gtk::SignalListItemFactory::new();
    let c = cfg.clone();
    factory.connect_setup(move |_, obj| {
        let li = obj.downcast_ref::<gtk::ListItem>().unwrap();
        let cell = ThumbCell::new(false);
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
    grid.set_max_columns(64);
    grid.set_min_columns(1);
    grid.set_enable_rubberband(true);
    grid.add_css_class("flowlin-grid");
    grid
}
