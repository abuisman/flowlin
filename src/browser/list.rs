//! List mode: one row per file with a small thumbnail and metadata columns.
//! Clicking a column header changes the tab-wide sort (same as the sort menu).

use std::cell::RefCell;
use std::rc::Rc;

use gtk::glib;
use gtk::pango;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use super::{ThumbCell, ViewConfig};
use crate::i18n::tr;
use crate::model::{BrowserModel, ImageItem, SortKey};

type Formatter = Box<dyn Fn(&ImageItem) -> String>;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct BoundLabel {
        pub label: gtk::Label,
        pub bound: RefCell<Option<(ImageItem, glib::SignalHandlerId)>>,
        pub format: RefCell<Option<Rc<Formatter>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for BoundLabel {
        const NAME: &'static str = "FlowlinBoundLabel";
        type Type = super::BoundLabel;
        type ParentType = gtk::Box;
    }

    impl ObjectImpl for BoundLabel {
        fn constructed(&self) {
            self.parent_constructed();
            self.label.set_xalign(0.0);
            self.label.set_ellipsize(pango::EllipsizeMode::Middle);
            self.label.set_hexpand(true);
            self.obj().append(&self.label);
        }
    }
    impl WidgetImpl for BoundLabel {}
    impl BoxImpl for BoundLabel {}
}

glib::wrapper! {
    /// A label that re-renders when its item changes.
    pub struct BoundLabel(ObjectSubclass<imp::BoundLabel>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl BoundLabel {
    fn new(format: Rc<Formatter>, numeric: bool) -> Self {
        let l: Self = glib::Object::new();
        *l.imp().format.borrow_mut() = Some(format);
        if numeric {
            l.imp().label.add_css_class("numeric");
            l.imp().label.set_xalign(1.0);
        }
        l
    }

    fn render(&self) {
        let imp = self.imp();
        if let (Some((item, _)), Some(f)) = (imp.bound.borrow().as_ref(), imp.format.borrow().as_ref()) {
            imp.label.set_label(&f(item));
        }
    }

    fn bind(&self, item: &ImageItem) {
        self.unbind();
        let weak = self.downgrade();
        let id = item.connect_changed(move |_| {
            if let Some(l) = weak.upgrade() {
                l.render();
            }
        });
        *self.imp().bound.borrow_mut() = Some((item.clone(), id));
        self.render();
    }

    fn unbind(&self) {
        if let Some((item, id)) = self.imp().bound.borrow_mut().take() {
            item.disconnect(id);
        }
    }
}

fn format_time(secs: i64) -> String {
    if secs == 0 {
        return String::new();
    }
    glib::DateTime::from_unix_local(secs)
        .and_then(|d| d.format("%Y-%m-%d %H:%M"))
        .map(|s| s.to_string())
        .unwrap_or_default()
}

pub struct ListView {
    pub view: gtk::ColumnView,
    columns: Vec<(gtk::ColumnViewColumn, SortKey)>,
    updating: Rc<std::cell::Cell<bool>>,
}

fn text_column(title: &str, numeric: bool, f: Formatter) -> gtk::ColumnViewColumn {
    let f = Rc::new(f);
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(move |_, obj| {
        let li = obj.downcast_ref::<gtk::ListItem>().unwrap();
        li.set_child(Some(&BoundLabel::new(f.clone(), numeric)));
    });
    factory.connect_bind(|_, obj| {
        let li = obj.downcast_ref::<gtk::ListItem>().unwrap();
        let l = li.child().and_downcast::<BoundLabel>().unwrap();
        l.bind(&li.item().and_downcast::<ImageItem>().unwrap());
    });
    factory.connect_unbind(|_, obj| {
        let li = obj.downcast_ref::<gtk::ListItem>().unwrap();
        if let Some(l) = li.child().and_downcast::<BoundLabel>() {
            l.unbind();
        }
    });
    let col = gtk::ColumnViewColumn::new(Some(title), Some(factory));
    col.set_resizable(true);
    col
}

impl ListView {
    pub fn new(model: &BrowserModel, on_sort: impl Fn(SortKey, bool) + 'static) -> Self {
        let cfg = Rc::new(ViewConfig::default());
        cfg.size.set(40);
        cfg.show_names.set(false);

        // Name column: small thumbnail + name.
        let factory = gtk::SignalListItemFactory::new();
        let c = cfg.clone();
        factory.connect_setup(move |_, obj| {
            let li = obj.downcast_ref::<gtk::ListItem>().unwrap();
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            let thumb = ThumbCell::new(false);
            c.register(&thumb);
            row.append(&thumb);
            let name: Formatter = Box::new(|i: &ImageItem| i.name());
            row.append(&BoundLabel::new(Rc::new(name), false));
            li.set_child(Some(&row));
        });
        let c = cfg.clone();
        factory.connect_bind(move |_, obj| {
            let li = obj.downcast_ref::<gtk::ListItem>().unwrap();
            let row = li.child().unwrap();
            let item = li.item().and_downcast::<ImageItem>().unwrap();
            let thumb = row.first_child().and_downcast::<ThumbCell>().unwrap();
            c.scale.set(row.scale_factor());
            thumb.bind(&item, &c);
            row.last_child().and_downcast::<BoundLabel>().unwrap().bind(&item);
        });
        factory.connect_unbind(|_, obj| {
            let li = obj.downcast_ref::<gtk::ListItem>().unwrap();
            if let Some(row) = li.child() {
                if let Some(t) = row.first_child().and_downcast::<ThumbCell>() {
                    t.unbind();
                }
                if let Some(l) = row.last_child().and_downcast::<BoundLabel>() {
                    l.unbind();
                }
            }
        });
        let name_col = gtk::ColumnViewColumn::new(Some(&tr("Name")), Some(factory));
        name_col.set_expand(true);
        name_col.set_resizable(true);

        let dims = text_column(
            &tr("Dimensions"),
            true,
            Box::new(|i| i.dimensions().map(|(w, h)| format!("{w} × {h}")).unwrap_or_default()),
        );
        let size = text_column(&tr("Size"), true, Box::new(|i| crate::util::format_size(i.size())));
        let modified = text_column(&tr("Modified"), true, Box::new(|i| format_time(i.mtime())));
        let kind = text_column(&tr("Type"), false, Box::new(|i| i.with_path(crate::fs::formats::type_label)));
        let rating = text_column(&tr("Rating"), false, Box::new(|i| "★".repeat(i.rating().unwrap_or(0) as usize)));
        let folder = text_column(&tr("Folder"), false, Box::new(|i| i.rel_dir()));

        let view = gtk::ColumnView::new(Some(model.selection.clone()));
        view.add_css_class("data-table");
        view.set_enable_rubberband(true);
        view.set_reorderable(false);
        let columns = vec![
            (name_col, SortKey::Name),
            (folder, SortKey::Folder),
            (dims, SortKey::Dimensions),
            (size, SortKey::Size),
            (modified, SortKey::Modified),
            (kind, SortKey::Type),
            (rating, SortKey::Name),
        ];
        for (i, (col, _)) in columns.iter().enumerate() {
            // Header-click sorting drives the shared model sorter; these
            // sorters only make the headers clickable.
            if i != 6 {
                col.set_sorter(Some(&gtk::CustomSorter::new(|_, _| gtk::Ordering::Equal)));
            }
            view.append_column(col);
        }
        let updating = Rc::new(std::cell::Cell::new(false));
        let cols: Vec<_> = columns.iter().map(|(c, k)| (c.clone(), *k)).collect();
        let u = updating.clone();
        if let Some(sorter) = view.sorter().and_downcast::<gtk::ColumnViewSorter>() {
            sorter.connect_changed(move |s, _| {
                if u.get() {
                    return;
                }
                if let Some(col) = s.primary_sort_column() {
                    if let Some((_, key)) = cols.iter().find(|(c, _)| *c == col) {
                        on_sort(*key, s.primary_sort_order() == gtk::SortType::Descending);
                    }
                }
            });
        }
        Self { view, columns, updating }
    }

    /// Reflect the current sort in the column headers.
    pub fn show_sort(&self, key: SortKey, descending: bool) {
        self.updating.set(true);
        let order = if descending { gtk::SortType::Descending } else { gtk::SortType::Ascending };
        let col = self.columns.iter().find(|(_, k)| *k == key).map(|(c, _)| c);
        self.view.sort_by_column(col, order);
        self.updating.set(false);
    }

    pub fn set_folder_column_visible(&self, v: bool) {
        self.columns[1].0.set_visible(v);
    }
}
