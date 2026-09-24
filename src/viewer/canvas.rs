//! The image canvas: draws one texture with fit / fill / 100 % / free zoom,
//! panning and view-only rotation. Downscaling uses trilinear (mipmapped)
//! filtering so detailed images do not moiré.

use std::cell::{Cell, RefCell};

use gtk::gdk;
use gtk::glib;
use gtk::graphene;
use gtk::gsk;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum ZoomMode {
    /// Fit inside the window, never enlarging past 100 %.
    #[default]
    Fit,
    /// Cover the window.
    Fill,
    /// One image pixel per device pixel.
    Actual,
    /// Free zoom (scale in logical px per image px).
    Manual(f64),
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct ImageCanvas {
        pub texture: RefCell<Option<gdk::Texture>>,
        /// Logical image size (original pixels, before rotation).
        pub img: Cell<(f64, f64)>,
        pub rotation: Cell<u8>,
        pub mode: Cell<ZoomMode>,
        pub offset: Cell<(f64, f64)>,
        pub drag_start: Cell<(f64, f64)>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ImageCanvas {
        const NAME: &'static str = "FlowlinImageCanvas";
        type Type = super::ImageCanvas;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("imagecanvas");
        }
    }

    impl ObjectImpl for ImageCanvas {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.set_hexpand(true);
            obj.set_vexpand(true);
            obj.set_overflow(gtk::Overflow::Hidden);
        }
    }

    impl WidgetImpl for ImageCanvas {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            let Some(tex) = self.texture.borrow().clone() else { return };
            let (w, h) = (obj.width() as f64, obj.height() as f64);
            let s = obj.scale();
            let (iw, ih) = self.img.get();
            let (ox, oy) = self.offset.get();
            let device_scale = s * obj.scale_factor() as f64;
            // Compare against the texture's own resolution (it may be pre-scaled).
            let tex_ratio = tex.width() as f64 / iw.max(1.0);
            let filter = if device_scale / tex_ratio < 1.0 {
                gsk::ScalingFilter::Trilinear
            } else if device_scale / tex_ratio >= 4.0 {
                gsk::ScalingFilter::Nearest
            } else {
                gsk::ScalingFilter::Linear
            };
            snapshot.save();
            snapshot.translate(&graphene::Point::new((w / 2.0 + ox) as f32, (h / 2.0 + oy) as f32));
            snapshot.rotate(90.0 * self.rotation.get() as f32);
            let bounds =
                graphene::Rect::new((-iw * s / 2.0) as f32, (-ih * s / 2.0) as f32, (iw * s) as f32, (ih * s) as f32);
            snapshot.append_scaled_texture(&tex, filter, &bounds);
            snapshot.restore();
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            self.parent_size_allocate(width, height, baseline);
            self.obj().clamp_offset();
        }
    }
}

glib::wrapper! {
    pub struct ImageCanvas(ObjectSubclass<imp::ImageCanvas>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for ImageCanvas {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl ImageCanvas {
    /// Show a new image. `keep_view` keeps zoom/pan (animation frames).
    pub fn set_image(&self, tex: Option<gdk::Texture>, width: u32, height: u32, keep_view: bool) {
        let imp = self.imp();
        *imp.texture.borrow_mut() = tex;
        imp.img.set((width.max(1) as f64, height.max(1) as f64));
        if !keep_view {
            imp.offset.set((0.0, 0.0));
            imp.rotation.set(0);
        }
        self.queue_draw();
    }

    pub fn has_image(&self) -> bool {
        self.imp().texture.borrow().is_some()
    }

    pub fn mode(&self) -> ZoomMode {
        self.imp().mode.get()
    }

    pub fn set_mode(&self, m: ZoomMode) {
        self.imp().mode.set(m);
        self.imp().offset.set((0.0, 0.0));
        self.queue_draw();
    }

    /// Image size as displayed (after rotation), logical image px.
    fn rotated(&self) -> (f64, f64) {
        let (iw, ih) = self.imp().img.get();
        if self.imp().rotation.get() % 2 == 1 {
            (ih, iw)
        } else {
            (iw, ih)
        }
    }

    fn fit_scale(&self) -> f64 {
        let (dw, dh) = self.rotated();
        let (w, h) = (self.width().max(1) as f64, self.height().max(1) as f64);
        (w / dw).min(h / dh)
    }

    fn actual_scale(&self) -> f64 {
        1.0 / self.scale_factor().max(1) as f64
    }

    /// Current scale in logical px per image px.
    pub fn scale(&self) -> f64 {
        match self.imp().mode.get() {
            ZoomMode::Fit => self.fit_scale().min(self.actual_scale()),
            ZoomMode::Fill => {
                let (dw, dh) = self.rotated();
                (self.width().max(1) as f64 / dw).max(self.height().max(1) as f64 / dh)
            }
            ZoomMode::Actual => self.actual_scale(),
            ZoomMode::Manual(s) => s,
        }
    }

    /// Zoom percentage relative to device pixels.
    pub fn zoom_percent(&self) -> f64 {
        self.scale() * self.scale_factor() as f64 * 100.0
    }

    /// Is the image bigger than the view (so it can be panned)?
    pub fn is_zoomed(&self) -> bool {
        let s = self.scale();
        let (dw, dh) = self.rotated();
        dw * s > self.width() as f64 + 0.5 || dh * s > self.height() as f64 + 0.5
    }

    /// Multiply the zoom by `factor`, keeping the image point under
    /// (`px`, `py`) fixed.
    pub fn zoom_at(&self, factor: f64, px: f64, py: f64) {
        let old = self.scale();
        let min = (self.fit_scale() * 0.5).min(0.02);
        let new = (old * factor).clamp(min, 32.0 * self.actual_scale());
        self.zoom_to(new, px, py);
    }

    fn zoom_to(&self, new: f64, px: f64, py: f64) {
        let old = self.scale();
        let (cx, cy) = (self.width() as f64 / 2.0, self.height() as f64 / 2.0);
        let (ox, oy) = self.imp().offset.get();
        let k = new / old;
        let nx = (px - cx) - (px - cx - ox) * k;
        let ny = (py - cy) - (py - cy - oy) * k;
        self.imp().mode.set(ZoomMode::Manual(new));
        self.imp().offset.set((nx, ny));
        self.clamp_offset();
        self.queue_draw();
    }

    /// Switch to 100 % around a point (long-press).
    pub fn actual_at(&self, px: f64, py: f64) {
        self.zoom_to(self.actual_scale(), px, py);
        self.imp().mode.set(ZoomMode::Actual);
    }

    pub fn rotate(&self, quarter_turns: i32) {
        let r = (self.imp().rotation.get() as i32 + quarter_turns).rem_euclid(4);
        self.imp().rotation.set(r as u8);
        self.clamp_offset();
        self.queue_draw();
    }

    pub fn begin_pan(&self) {
        self.imp().drag_start.set(self.imp().offset.get());
    }

    pub fn pan_to(&self, dx: f64, dy: f64) {
        let (sx, sy) = self.imp().drag_start.get();
        self.imp().offset.set((sx + dx, sy + dy));
        self.clamp_offset();
        self.queue_draw();
    }

    pub fn pan_by(&self, dx: f64, dy: f64) {
        let (ox, oy) = self.imp().offset.get();
        self.imp().offset.set((ox + dx, oy + dy));
        self.clamp_offset();
        self.queue_draw();
    }

    fn clamp_offset(&self) {
        let s = self.scale();
        let (dw, dh) = self.rotated();
        let mx = ((dw * s - self.width() as f64) / 2.0).max(0.0);
        let my = ((dh * s - self.height() as f64) / 2.0).max(0.0);
        let (ox, oy) = self.imp().offset.get();
        self.imp().offset.set((ox.clamp(-mx, mx), oy.clamp(-my, my)));
    }
}
