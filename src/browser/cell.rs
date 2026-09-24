//! A thumbnail cell: rounded card with the image, file name beneath and an
//! optional relative-folder label (recursive mode).

use std::cell::{Cell, RefCell};

use gtk::glib;
use gtk::pango;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use super::ViewConfig;
use crate::model::{ImageItem, ThumbState};
use crate::thumbs::ThumbService;

/// Diagnostics: total cell widgets constructed.
pub static CELLS_CREATED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

mod frame_imp {
    use super::*;

    /// Reports exactly the requested size regardless of the picture's
    /// natural size, so every card in a row is identical.
    #[derive(Default)]
    pub struct Frame {
        pub size: Cell<(i32, i32)>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Frame {
        const NAME: &'static str = "FlowlinThumbFrame";
        type Type = super::Frame;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for Frame {
        fn dispose(&self) {
            while let Some(c) = self.obj().first_child() {
                c.unparent();
            }
        }
    }

    impl WidgetImpl for Frame {
        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let (w, h) = self.size.get();
            let v = if orientation == gtk::Orientation::Horizontal { w } else { h };
            (v, v, -1, -1)
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            if let Some(c) = self.obj().first_child() {
                c.allocate(width, height, baseline, None);
            }
        }
    }
}

glib::wrapper! {
    pub struct Frame(ObjectSubclass<frame_imp::Frame>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Frame {
    fn set_size(&self, w: i32, h: i32) {
        if self.imp().size.replace((w, h)) != (w, h) {
            self.queue_resize();
        }
    }
}

impl Default for Frame {
    fn default() -> Self {
        glib::Object::new()
    }
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct ThumbCell {
        pub frame: Frame,
        pub card: gtk::Overlay,
        pub picture: gtk::Picture,
        pub broken: gtk::Image,
        pub stars: gtk::Label,
        pub play: gtk::Image,
        pub name: gtk::Label,
        pub folder: gtk::Label,
        pub item: RefCell<Option<(ImageItem, glib::SignalHandlerId)>>,
        /// Waterfall cells keep the aspect ratio instead of a square card.
        pub free_height: Cell<bool>,
        pub size: Cell<i32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ThumbCell {
        const NAME: &'static str = "FlowlinThumbCell";
        type Type = super::ThumbCell;
        type ParentType = gtk::Box;
    }

    impl ObjectImpl for ThumbCell {
        fn constructed(&self) {
            self.parent_constructed();
            super::CELLS_CREATED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let obj = self.obj();
            obj.set_orientation(gtk::Orientation::Vertical);
            obj.set_spacing(4);
            obj.add_css_class("thumb-cell");

            self.picture.set_content_fit(gtk::ContentFit::Contain);
            self.picture.set_can_shrink(true);
            self.picture.add_css_class("thumb-picture");
            self.card.set_child(Some(&self.picture));
            self.card.add_css_class("thumb-card");
            self.card.set_overflow(gtk::Overflow::Hidden);
            self.card.set_parent(&self.frame);
            self.frame.set_halign(gtk::Align::Center);

            self.broken.set_icon_name(Some("image-missing-symbolic"));
            self.broken.set_pixel_size(32);
            self.broken.add_css_class("dim-label");
            self.broken.set_visible(false);
            self.card.add_overlay(&self.broken);

            self.play.set_icon_name(Some("media-playback-start-symbolic"));
            self.play.set_pixel_size(20);
            self.play.add_css_class("thumb-play");
            self.play.set_halign(gtk::Align::Center);
            self.play.set_valign(gtk::Align::Center);
            self.play.set_visible(false);
            self.play.set_can_target(false);
            self.card.add_overlay(&self.play);

            self.stars.add_css_class("thumb-stars");
            self.stars.set_halign(gtk::Align::Start);
            self.stars.set_valign(gtk::Align::End);
            self.stars.set_visible(false);
            self.card.add_overlay(&self.stars);

            for (l, class) in [(&self.name, "thumb-name"), (&self.folder, "thumb-folder")] {
                l.set_ellipsize(pango::EllipsizeMode::Middle);
                l.set_single_line_mode(true);
                l.set_max_width_chars(1);
                l.set_hexpand(true);
                l.add_css_class(class);
            }
            self.folder.add_css_class("dim-label");
            self.folder.add_css_class("caption");
            self.name.add_css_class("caption");

            obj.append(&self.frame);
            obj.append(&self.name);
            obj.append(&self.folder);
        }

        fn dispose(&self) {
            self.obj().unbind();
        }
    }

    impl WidgetImpl for ThumbCell {}
    impl BoxImpl for ThumbCell {}
}

glib::wrapper! {
    pub struct ThumbCell(ObjectSubclass<imp::ThumbCell>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl Default for ThumbCell {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl ThumbCell {
    pub fn new(free_height: bool) -> Self {
        let c = Self::default();
        c.imp().free_height.set(free_height);
        c
    }

    pub fn item(&self) -> Option<ImageItem> {
        self.imp().item.borrow().as_ref().map(|(i, _)| i.clone())
    }

    /// Size an unbound cell like a real one. GtkGridView estimates row
    /// heights from fresh cells; if they measured 0 px it would create a
    /// widget for thousands of items at once.
    pub fn apply_size(&self, cfg: &ViewConfig) {
        let imp = self.imp();
        imp.size.set(cfg.size.get());
        self.update_card_size();
        imp.name.set_visible(cfg.show_names.get());
        imp.folder.set_visible(cfg.show_folders.get());
        if imp.name.label().is_empty() {
            imp.name.set_label(" ");
            imp.folder.set_label(" ");
        }
    }

    pub fn apply_config(&self, cfg: &ViewConfig) {
        let imp = self.imp();
        let size = cfg.size.get();
        imp.size.set(size);
        self.update_card_size();
        imp.name.set_visible(cfg.show_names.get());
        imp.folder.set_visible(cfg.show_folders.get());
        if let Some(item) = self.item() {
            // Files directly in the scanned folder show that folder's name.
            let mut folder = item.rel_dir();
            if folder.is_empty() {
                folder = item.with_path(|p| p.parent().map(crate::util::file_name).unwrap_or_default());
            }
            imp.folder.set_label(&folder);
        }
        if let Some(item) = self.item() {
            ThumbService::get().request(&item, cfg.thumb_px());
        }
    }

    /// Waterfall columns stretch to fill the width; override the cell width.
    pub fn set_width_override(&self, w: i32) {
        if self.imp().size.get() != w {
            self.imp().size.set(w);
            self.update_card_size();
        }
    }

    fn update_card_size(&self) {
        let imp = self.imp();
        let size = imp.size.get();
        let h = if imp.free_height.get() {
            let aspect = self.item().map(|i| i.aspect()).unwrap_or(1.0);
            (size as f64 * aspect).round() as i32
        } else {
            size
        };
        imp.frame.set_size(size, h);
        imp.name.set_size_request(size, -1);
        imp.folder.set_size_request(size, -1);
    }

    pub fn bind(&self, item: &ImageItem, cfg: &ViewConfig) {
        self.unbind();
        let imp = self.imp();
        item.bind_ref();
        let weak = self.downgrade();
        let id = item.connect_changed(move |_| {
            if let Some(c) = weak.upgrade() {
                c.refresh(true);
            }
        });
        *imp.item.borrow_mut() = Some((item.clone(), id));
        let name = item.name();
        imp.name.set_label(&name);
        self.set_tooltip_text(Some(&name));
        imp.picture.remove_css_class("loaded");
        imp.picture.add_css_class("instant");
        self.apply_config(cfg);
        self.refresh(false);
    }

    pub fn unbind(&self) {
        let imp = self.imp();
        if let Some((item, id)) = imp.item.borrow_mut().take() {
            item.disconnect(id);
            item.bind_unref();
            ThumbService::get().release(&item);
        }
        imp.picture.set_paintable(None::<&gtk::gdk::Paintable>);
        imp.picture.remove_css_class("loaded");
        imp.broken.set_visible(false);
        imp.stars.set_visible(false);
        imp.play.set_visible(false);
    }

    fn refresh(&self, animate: bool) {
        let imp = self.imp();
        let Some(item) = self.item() else { return };
        let tex = item.texture();
        let has = tex.is_some();
        imp.picture.set_paintable(tex.as_ref());
        if has {
            if animate {
                imp.picture.remove_css_class("instant");
            }
            imp.picture.add_css_class("loaded");
        }
        imp.broken.set_visible(item.thumb_state() == ThumbState::Failed);
        imp.play.set_visible(item.is_video());
        match item.rating() {
            Some(r) if r > 0 => {
                imp.stars.set_label(&"★".repeat(r as usize));
                imp.stars.set_visible(true);
            }
            _ => imp.stars.set_visible(false),
        }
        let name = item.name();
        if imp.name.label() != name {
            imp.name.set_label(&name);
            self.set_tooltip_text(Some(&name));
        }
        if imp.free_height.get() {
            self.update_card_size();
        }
    }
}
