//! Full-size viewer, shown as an overlay inside a tab so the tab's grid
//! state is preserved. Steps through the tab's current (sorted, filtered,
//! possibly recursive) model.

mod canvas;
mod loader;

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::Duration;

use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;

pub use canvas::{ImageCanvas, ZoomMode};
use loader::{Loaded, Loader};

use crate::i18n::tr;
use crate::model::{BrowserModel, ImageItem};
use crate::util::{format_count, format_size};

type ItemCallback = Box<dyn Fn(&ImageItem)>;
type RateCallback = Box<dyn Fn(&ImageItem, u8)>;
/// (current index, rows down (+) or up (-)) -> index of the item there.
type RowCallback = Box<dyn Fn(u32, i32) -> Option<u32>>;
type ShowCallback = Box<dyn Fn(u32)>;

struct Anim {
    loaded: Rc<Loaded>,
    frame: usize,
    source: Option<glib::SourceId>,
}

pub struct Inner {
    pub root: gtk::Overlay,
    canvas: ImageCanvas,
    /// Video playback surface (GtkMediaFile via GTK's GStreamer backend).
    video: gtk::Picture,
    media: RefCell<Option<gtk::MediaFile>>,
    /// Play/pause, seek bar and volume for videos.
    controls: gtk::MediaControls,
    controls_rev: gtk::Revealer,
    hud: gtk::Revealer,
    hud_name: gtk::Label,
    hud_info: gtk::Label,
    spinner: gtk::Spinner,
    error: gtk::Box,
    model: BrowserModel,
    loader: Loader,
    current: RefCell<Option<ImageItem>>,
    index: Cell<u32>,
    open: Cell<bool>,
    /// Zoom mode the user picked explicitly (sticky across images).
    sticky: Cell<ZoomMode>,
    hud_pinned: Cell<bool>,
    hud_timer: RefCell<Option<glib::SourceId>>,
    slideshow: RefCell<Option<glib::SourceId>>,
    anim: RefCell<Option<Anim>>,
    pointer: Cell<(f64, f64)>,
    scroll_acc: Cell<f64>,
    on_close: RefCell<Option<ItemCallback>>,
    on_trash: RefCell<Option<ItemCallback>>,
    on_rate: RefCell<Option<RateCallback>>,
    on_row: RefCell<Option<RowCallback>>,
    on_show: RefCell<Option<ShowCallback>>,
}

#[derive(Clone)]
pub struct Viewer(Rc<Inner>);

impl Viewer {
    pub fn new(model: &BrowserModel) -> Self {
        let root = gtk::Overlay::new();
        root.add_css_class("viewer");
        root.set_focusable(true);
        root.set_visible(false);
        let canvas = ImageCanvas::default();
        canvas.set_focusable(true);
        root.set_child(Some(&canvas));
        let video = gtk::Picture::new();
        video.set_content_fit(gtk::ContentFit::Contain);
        video.set_can_shrink(true);
        // Input goes to the canvas underneath (gestures, scrolling).
        video.set_can_target(false);
        video.set_visible(false);
        root.add_overlay(&video);

        let spinner = gtk::Spinner::new();
        spinner.set_size_request(32, 32);
        spinner.set_halign(gtk::Align::Center);
        spinner.set_valign(gtk::Align::Center);
        spinner.set_visible(false);
        root.add_overlay(&spinner);

        let error = gtk::Box::new(gtk::Orientation::Vertical, 12);
        error.set_halign(gtk::Align::Center);
        error.set_valign(gtk::Align::Center);
        let icon = gtk::Image::from_icon_name("image-missing-symbolic");
        icon.set_pixel_size(64);
        error.append(&icon);
        let msg = gtk::Label::new(Some(&tr("This image cannot be displayed")));
        msg.add_css_class("title-3");
        error.append(&msg);
        error.add_css_class("viewer-error");
        error.set_visible(false);
        root.add_overlay(&error);

        let hud_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
        hud_box.add_css_class("osd");
        hud_box.add_css_class("viewer-hud");
        let hud_name = gtk::Label::new(None);
        hud_name.add_css_class("heading");
        hud_name.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        hud_name.set_max_width_chars(80);
        let hud_info = gtk::Label::new(None);
        hud_info.add_css_class("caption");
        hud_info.add_css_class("numeric");
        hud_box.append(&hud_name);
        hud_box.append(&hud_info);
        let hud = gtk::Revealer::new();
        hud.set_transition_type(gtk::RevealerTransitionType::Crossfade);
        hud.set_child(Some(&hud_box));
        hud.set_halign(gtk::Align::Center);
        hud.set_valign(gtk::Align::End);
        hud.set_margin_bottom(18);
        hud.set_can_target(false);
        root.add_overlay(&hud);

        let controls = gtk::MediaControls::new(None::<&gtk::MediaStream>);
        controls.set_hexpand(true);
        let controls_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        controls_box.add_css_class("osd");
        controls_box.add_css_class("viewer-controls");
        controls_box.append(&controls);
        let clamp = adw::Clamp::new();
        clamp.set_maximum_size(720);
        clamp.set_child(Some(&controls_box));
        let controls_rev = gtk::Revealer::new();
        controls_rev.set_transition_type(gtk::RevealerTransitionType::Crossfade);
        controls_rev.set_child(Some(&clamp));
        controls_rev.set_valign(gtk::Align::End);
        controls_rev.set_margin_bottom(12);
        controls_rev.set_margin_start(12);
        controls_rev.set_margin_end(12);
        root.add_overlay(&controls_rev);

        let v = Viewer(Rc::new(Inner {
            root,
            canvas,
            video,
            media: Default::default(),
            controls,
            controls_rev,
            hud,
            hud_name,
            hud_info,
            spinner,
            error,
            model: model.clone(),
            loader: Loader::new(),
            current: Default::default(),
            index: Cell::new(0),
            open: Cell::new(false),
            sticky: Cell::new(ZoomMode::Fit),
            hud_pinned: Cell::new(false),
            hud_timer: Default::default(),
            slideshow: Default::default(),
            anim: Default::default(),
            pointer: Cell::new((0.0, 0.0)),
            scroll_acc: Cell::new(0.0),
            on_close: Default::default(),
            on_trash: Default::default(),
            on_rate: Default::default(),
            on_row: Default::default(),
            on_show: Default::default(),
        }));
        let w = v.downgrade();
        v.0.loader.connect_loaded(move |p| {
            if let Some(v) = upgrade(&w) {
                v.loaded(p);
            }
        });
        let w = v.downgrade();
        model.sort_model.connect_items_changed(move |_, _, _, _| {
            if let Some(v) = upgrade(&w) {
                v.model_changed();
            }
        });
        v.setup_input();
        v
    }

    fn downgrade(&self) -> Weak<Inner> {
        Rc::downgrade(&self.0)
    }

    pub fn widget(&self) -> &gtk::Overlay {
        &self.0.root
    }

    pub fn is_open(&self) -> bool {
        self.0.open.get()
    }

    pub fn current(&self) -> Option<ImageItem> {
        self.0.current.borrow().clone()
    }

    pub fn connect_close(&self, f: impl Fn(&ImageItem) + 'static) {
        *self.0.on_close.borrow_mut() = Some(Box::new(f));
    }

    pub fn connect_trash(&self, f: impl Fn(&ImageItem) + 'static) {
        *self.0.on_trash.borrow_mut() = Some(Box::new(f));
    }

    pub fn connect_rate(&self, f: impl Fn(&ImageItem, u8) + 'static) {
        *self.0.on_rate.borrow_mut() = Some(Box::new(f));
    }

    /// Up/Down move by one row of the thumbnail layout behind the viewer.
    pub fn connect_row_step(&self, f: impl Fn(u32, i32) -> Option<u32> + 'static) {
        *self.0.on_row.borrow_mut() = Some(Box::new(f));
    }

    /// Called whenever another image is shown (keeps the grid in sync).
    pub fn connect_show(&self, f: impl Fn(u32) + 'static) {
        *self.0.on_show.borrow_mut() = Some(Box::new(f));
    }

    fn row_step(&self, rows: i32) {
        let target = self.0.on_row.borrow().as_ref().and_then(|f| f(self.0.index.get(), rows));
        if let Some(t) = target {
            if t != self.0.index.get() {
                self.show(t);
            }
        }
    }

    pub fn open(&self, pos: u32) {
        let i = &self.0;
        i.open.set(true);
        i.root.set_visible(true);
        i.sticky.set(ZoomMode::Fit);
        self.show(pos);
        i.canvas.grab_focus();
        self.flash_hud();
    }

    pub fn close(&self) {
        let i = &self.0;
        if !i.open.get() {
            return;
        }
        self.stop_slideshow();
        self.stop_animation();
        self.stop_video();
        i.open.set(false);
        i.root.set_visible(false);
        i.canvas.set_image(None, 1, 1, false);
        let cur = i.current.borrow_mut().take();
        if let (Some(item), Some(f)) = (cur, i.on_close.borrow().as_ref()) {
            f(&item);
        }
    }

    fn show(&self, pos: u32) {
        let i = &self.0;
        let n = i.model.n_items();
        if n == 0 {
            self.close();
            return;
        }
        let pos = pos.min(n - 1);
        let Some(item) = i.model.item(pos) else { return };
        i.index.set(pos);
        if let Some(f) = i.on_show.borrow().as_ref() {
            f(pos);
        }
        *i.current.borrow_mut() = Some(item.clone());
        self.stop_animation();
        self.stop_video();
        let path = item.path();
        if item.is_video() {
            self.play_video(&path);
        } else {
            match i.loader.get(&path, item.mtime()) {
                Some(res) => self.display(res),
                None => {
                    // Show the thumbnail scaled up while the full image decodes.
                    let (w, h) = item.dimensions().unwrap_or((1, 1));
                    i.canvas.set_image(item.texture(), w, h, false);
                    i.canvas.set_mode(i.sticky.get());
                    i.error.set_visible(false);
                    i.spinner.set_visible(true);
                    i.spinner.start();
                    i.loader.request(&path, item.mtime());
                }
            }
        }
        // Prefetch neighbouring images.
        for p in [pos + 1, pos.wrapping_sub(1), pos + 2] {
            if p < n {
                if let Some(it) = i.model.item(p).filter(|it| !it.is_video()) {
                    i.loader.request(&it.path(), it.mtime());
                }
            }
        }
        self.update_hud();
    }

    fn display(&self, res: loader::LoadResult) {
        let i = &self.0;
        i.spinner.stop();
        i.spinner.set_visible(false);
        match res {
            Ok(l) => {
                i.error.set_visible(false);
                i.canvas.set_image(Some(l.frames[0].0.clone()), l.width, l.height, false);
                i.canvas.set_mode(i.sticky.get());
                if let Some(item) = self.current() {
                    item.set_dimensions(l.width, l.height);
                }
                if l.frames.len() > 1 {
                    *i.anim.borrow_mut() = Some(Anim { loaded: l, frame: 0, source: None });
                    self.schedule_frame();
                }
            }
            Err(_) => {
                i.canvas.set_image(None, 1, 1, false);
                i.error.set_visible(true);
            }
        }
        self.update_hud();
    }

    fn loaded(&self, path: &Path) {
        let Some(item) = self.current() else { return };
        if !self.is_open() || item.with_path(|p| p != path) {
            return;
        }
        if let Some(res) = self.0.loader.get(path, item.mtime()) {
            self.display(res);
        }
    }

    fn play_video(&self, path: &Path) {
        let i = &self.0;
        i.canvas.set_image(None, 1, 1, false);
        i.error.set_visible(false);
        i.spinner.stop();
        i.spinner.set_visible(false);
        let media = gtk::MediaFile::for_filename(path);
        media.set_muted(true);
        media.set_loop(true);
        let w = self.downgrade();
        media.connect_error_notify(move |m| {
            if let (Some(v), Some(e)) = (upgrade(&w), m.error()) {
                tracing::info!("cannot play video: {e}");
                v.0.video.set_visible(false);
                v.0.error.set_visible(true);
            }
        });
        let w = self.downgrade();
        media.connect_duration_notify(move |_| {
            if let Some(v) = upgrade(&w) {
                v.update_hud();
            }
        });
        media.play();
        i.video.set_paintable(Some(&media));
        i.video.set_visible(true);
        i.controls.set_media_stream(Some(&media));
        i.controls_rev.set_reveal_child(true);
        // Keep the info HUD above the controls bar.
        i.hud.set_margin_bottom(84);
        let w = self.downgrade();
        media.connect_playing_notify(move |m| {
            if let Some(v) = upgrade(&w) {
                if !m.is_playing() {
                    v.0.controls_rev.set_reveal_child(true);
                }
            }
        });
        *i.media.borrow_mut() = Some(media);
    }

    fn stop_video(&self) {
        let i = &self.0;
        if let Some(m) = i.media.borrow_mut().take() {
            m.pause();
            m.clear();
        }
        i.video.set_paintable(None::<&gtk::gdk::Paintable>);
        i.video.set_visible(false);
        i.controls.set_media_stream(None::<&gtk::MediaStream>);
        i.controls_rev.set_reveal_child(false);
        i.hud.set_margin_bottom(18);
    }

    /// Change the volume by `delta` (0–1); raising it unmutes.
    fn change_volume(&self, delta: f64) {
        let Some(m) = self.media() else { return };
        let v = (m.volume() + delta).clamp(0.0, 1.0);
        m.set_volume(v);
        m.set_muted(v <= 0.0);
        self.update_hud();
        self.flash_hud();
    }

    fn media(&self) -> Option<gtk::MediaFile> {
        self.0.media.borrow().clone()
    }

    /// Seek the playing video by `secs` seconds.
    fn seek_by(&self, secs: i64) {
        if let Some(m) = self.media() {
            let t = (m.timestamp() + secs * 1_000_000).clamp(0, m.duration().max(0));
            m.seek(t);
            self.flash_hud();
        }
    }

    fn schedule_frame(&self) {
        let delay = {
            let a = self.0.anim.borrow();
            let Some(a) = a.as_ref() else { return };
            a.loaded.frames[a.frame].1.max(20)
        };
        let w = self.downgrade();
        let id = glib::timeout_add_local_once(Duration::from_millis(delay as u64), move || {
            let Some(v) = upgrade(&w) else { return };
            let tex = {
                let mut a = v.0.anim.borrow_mut();
                let Some(a) = a.as_mut() else { return };
                a.source = None;
                a.frame = (a.frame + 1) % a.loaded.frames.len();
                (a.loaded.frames[a.frame].0.clone(), a.loaded.width, a.loaded.height)
            };
            v.0.canvas.set_image(Some(tex.0), tex.1, tex.2, true);
            v.schedule_frame();
        });
        if let Some(a) = self.0.anim.borrow_mut().as_mut() {
            a.source = Some(id);
        }
    }

    fn stop_animation(&self) {
        if let Some(mut a) = self.0.anim.borrow_mut().take() {
            if let Some(s) = a.source.take() {
                s.remove();
            }
        }
    }

    /// Keep showing the same item after the model changed (sorting, files
    /// removed); if it is gone, show whatever now sits at its index.
    fn model_changed(&self) {
        if !self.is_open() {
            return;
        }
        let i = &self.0;
        let n = i.model.n_items();
        if n == 0 {
            self.close();
            return;
        }
        let cur = self.current();
        let idx = i.index.get();
        if let Some(item) = &cur {
            if i.model.item(idx.min(n - 1)).as_ref() == Some(item) {
                i.index.set(idx.min(n - 1));
                self.update_hud();
                return;
            }
            if let Some(p) = i.model.position_of(item) {
                i.index.set(p);
                self.update_hud();
                return;
            }
        }
        self.show(idx.min(n - 1));
    }

    pub fn step(&self, delta: i32) {
        let n = self.0.model.n_items() as i64;
        if n == 0 {
            return;
        }
        let next = (self.0.index.get() as i64 + delta as i64).clamp(0, n - 1) as u32;
        if next != self.0.index.get() {
            self.show(next);
        }
    }

    fn update_hud(&self) {
        self.update_hud_now();
        // The zoom % depends on the canvas size, which is only known once
        // the (just shown) viewer has been laid out.
        let w = self.downgrade();
        glib::idle_add_local_once(move || {
            if let Some(v) = upgrade(&w) {
                v.update_hud_now();
            }
        });
    }

    fn update_hud_now(&self) {
        let i = &self.0;
        let Some(item) = self.current() else { return };
        i.hud_name.set_label(&item.name());
        let mut parts = vec![format!(
            "{} / {}",
            format_count(i.index.get() as usize + 1),
            format_count(i.model.n_items() as usize)
        )];
        if let Some(m) = self.media() {
            let secs = m.duration() / 1_000_000;
            if secs > 0 {
                parts.push(format!("{}:{:02}", secs / 60, secs % 60));
            }
            parts.push(if m.is_muted() {
                tr("Muted")
            } else {
                format!("{} {:.0} %", tr("Volume"), m.volume() * 100.0)
            });
        } else if let Some((w, h)) = item.dimensions() {
            parts.push(format!("{w} × {h}"));
        }
        parts.push(format_size(item.size()));
        if i.canvas.has_image() && i.media.borrow().is_none() {
            parts.push(format!("{:.0} %", i.canvas.zoom_percent()));
        }
        if i.slideshow.borrow().is_some() {
            parts.push(tr("Slideshow"));
        }
        i.hud_info.set_label(&parts.join("  ·  "));
    }

    fn flash_hud(&self) {
        let i = &self.0;
        i.hud.set_reveal_child(true);
        if i.media.borrow().is_some() {
            i.controls_rev.set_reveal_child(true);
        }
        if let Some(t) = i.hud_timer.borrow_mut().take() {
            t.remove();
        }
        if i.hud_pinned.get() {
            return;
        }
        let w = self.downgrade();
        let id = glib::timeout_add_local_once(Duration::from_secs(2), move || {
            if let Some(v) = upgrade(&w) {
                v.0.hud_timer.borrow_mut().take();
                if !v.0.hud_pinned.get() {
                    v.0.hud.set_reveal_child(false);
                }
                // Hide video controls only while playing and not hovered.
                let playing = v.media().is_some_and(|m| m.is_playing());
                if playing && !v.pointer_over_controls() {
                    v.0.controls_rev.set_reveal_child(false);
                }
            }
        });
        *i.hud_timer.borrow_mut() = Some(id);
    }

    fn pointer_over_controls(&self) -> bool {
        let (x, y) = self.0.pointer.get();
        let c = &self.0.controls_rev;
        c.compute_bounds(&self.0.root).is_some_and(|b| {
            let (x, y) = (x as f32, y as f32);
            x >= b.x() && x <= b.x() + b.width() && y >= b.y() && y <= b.y() + b.height()
        })
    }

    fn toggle_hud(&self) {
        let pinned = !self.0.hud_pinned.get();
        self.0.hud_pinned.set(pinned);
        self.0.hud.set_reveal_child(pinned);
        if pinned {
            if let Some(t) = self.0.hud_timer.borrow_mut().take() {
                t.remove();
            }
        }
    }

    fn set_zoom_mode(&self, m: ZoomMode) {
        self.0.sticky.set(m);
        self.0.canvas.set_mode(m);
        self.update_hud();
        self.flash_hud();
    }

    fn zoom(&self, factor: f64, at_pointer: bool) {
        let c = &self.0.canvas;
        let (x, y) = if at_pointer { self.0.pointer.get() } else { (c.width() as f64 / 2.0, c.height() as f64 / 2.0) };
        c.zoom_at(factor, x, y);
        self.update_hud();
        self.flash_hud();
    }

    pub fn toggle_slideshow(&self) {
        if self.0.slideshow.borrow().is_some() {
            self.stop_slideshow();
        } else {
            let secs = crate::settings::settings().uint("slideshow-interval").max(1);
            let w = self.downgrade();
            let id = glib::timeout_add_local(Duration::from_secs(secs as u64), move || {
                let Some(v) = upgrade(&w) else { return glib::ControlFlow::Break };
                let n = v.0.model.n_items();
                if n == 0 || !v.is_open() {
                    v.0.slideshow.borrow_mut().take();
                    return glib::ControlFlow::Break;
                }
                let next = (v.0.index.get() + 1) % n;
                v.show(next);
                glib::ControlFlow::Continue
            });
            *self.0.slideshow.borrow_mut() = Some(id);
        }
        self.update_hud();
        self.flash_hud();
    }

    fn stop_slideshow(&self) {
        if let Some(id) = self.0.slideshow.borrow_mut().take() {
            id.remove();
        }
    }

    fn setup_input(&self) {
        let i = &self.0;
        let keys = gtk::EventControllerKey::new();
        let w = self.downgrade();
        keys.connect_key_pressed(move |_, key, _, state| {
            use gdk::Key;
            let Some(v) = upgrade(&w) else { return glib::Propagation::Proceed };
            let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
            let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
            let alt = state.contains(gdk::ModifierType::ALT_MASK);
            if alt
                || (ctrl
                    && !matches!(key, Key::plus | Key::equal | Key::minus | Key::KP_Add | Key::KP_Subtract | Key::_0))
            {
                return glib::Propagation::Proceed;
            }
            // W/A/S/D act as the arrow keys in the viewer.
            let key = match key {
                Key::w | Key::W if !ctrl => Key::Up,
                Key::a | Key::A if !ctrl => Key::Left,
                Key::s | Key::S if !ctrl => Key::Down,
                Key::d | Key::D if !ctrl => Key::Right,
                k => k,
            };
            if let Some(m) = v.media() {
                match key {
                    Key::k | Key::K => {
                        if m.is_playing() {
                            m.pause();
                        } else {
                            m.play();
                        }
                        v.flash_hud();
                        return glib::Propagation::Stop;
                    }
                    // Arrows work inside the video: seek and volume.
                    // Page Up/Down (or the mouse side buttons) switch files.
                    Key::Left | Key::KP_Left => {
                        v.seek_by(if shift { -30 } else { -5 });
                        return glib::Propagation::Stop;
                    }
                    Key::Right | Key::KP_Right => {
                        v.seek_by(if shift { 30 } else { 5 });
                        return glib::Propagation::Stop;
                    }
                    Key::Up | Key::KP_Up => {
                        v.change_volume(0.1);
                        return glib::Propagation::Stop;
                    }
                    Key::Down | Key::KP_Down => {
                        v.change_volume(-0.1);
                        return glib::Propagation::Stop;
                    }
                    Key::m | Key::M => {
                        m.set_muted(!m.is_muted());
                        v.update_hud();
                        v.flash_hud();
                        return glib::Propagation::Stop;
                    }
                    _ => {}
                }
            }
            match key {
                Key::Right | Key::KP_Right | Key::Page_Down | Key::KP_Page_Down => v.step(1),
                Key::Left | Key::KP_Left | Key::BackSpace | Key::Page_Up | Key::KP_Page_Up => v.step(-1),
                Key::Up | Key::KP_Up => v.row_step(-1),
                Key::Down | Key::KP_Down => v.row_step(1),
                Key::Home | Key::KP_Home => v.show(0),
                Key::End | Key::KP_End => v.show(v.0.model.n_items().saturating_sub(1)),
                Key::Escape | Key::Return | Key::KP_Enter | Key::space => v.close(),
                Key::plus | Key::equal | Key::KP_Add => v.zoom(1.25, false),
                Key::minus | Key::KP_Subtract => v.zoom(0.8, false),
                Key::_0 | Key::KP_0 if ctrl => v.set_zoom_mode(ZoomMode::Fit),
                Key::_1 | Key::KP_1 if !shift => v.set_zoom_mode(ZoomMode::Actual),
                Key::_0 | Key::KP_0 | Key::f => v.set_zoom_mode(ZoomMode::Fit),
                Key::F => v.set_zoom_mode(ZoomMode::Fill),
                Key::i | Key::I => v.toggle_hud(),
                Key::r => v.0.canvas.rotate(1),
                Key::R => v.0.canvas.rotate(-1),
                Key::p | Key::P => v.toggle_slideshow(),
                Key::Delete | Key::KP_Delete if !shift => {
                    if let (Some(item), Some(f)) = (v.current(), v.0.on_trash.borrow().as_ref()) {
                        f(&item);
                    }
                }
                // Ratings: Shift+1…5 (1 is 100 %), Shift+0 clears. Keys 2–5 also work unshifted.
                Key::exclam
                | Key::at
                | Key::numbersign
                | Key::dollar
                | Key::percent
                | Key::parenright
                | Key::_2
                | Key::_3
                | Key::_4
                | Key::_5 => {
                    let stars = match key {
                        Key::exclam => 1,
                        Key::at | Key::_2 => 2,
                        Key::numbersign | Key::_3 => 3,
                        Key::dollar | Key::_4 => 4,
                        Key::percent | Key::_5 => 5,
                        _ => 0,
                    };
                    if let (Some(item), Some(f)) = (v.current(), v.0.on_rate.borrow().as_ref()) {
                        f(&item, stars);
                        v.flash_hud();
                    }
                }
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
        i.root.add_controller(keys);

        let motion = gtk::EventControllerMotion::new();
        let w = self.downgrade();
        motion.connect_motion(move |_, x, y| {
            let Some(v) = upgrade(&w) else { return };
            let (px, py) = v.0.pointer.get();
            v.0.pointer.set((x, y));
            if (px - x).abs() + (py - y).abs() > 2.0 {
                v.flash_hud();
            }
        });
        i.root.add_controller(motion);

        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
        let w = self.downgrade();
        scroll.connect_scroll(move |c, dx, dy| {
            let Some(v) = upgrade(&w) else { return glib::Propagation::Proceed };
            let ctrl = c.current_event_state().contains(gdk::ModifierType::CONTROL_MASK);
            if ctrl {
                v.zoom(1.1f64.powf(-dy), true);
            } else if v.0.canvas.is_zoomed() {
                v.0.canvas.pan_by(-dx * 40.0, -dy * 40.0);
            } else {
                let acc = v.0.scroll_acc.get() + dy + dx;
                if acc.abs() >= 1.0 {
                    v.step(if acc > 0.0 { 1 } else { -1 });
                    v.0.scroll_acc.set(0.0);
                } else {
                    v.0.scroll_acc.set(acc);
                }
            }
            glib::Propagation::Stop
        });
        i.canvas.add_controller(scroll);

        let drag = gtk::GestureDrag::new();
        let w = self.downgrade();
        drag.connect_drag_begin(move |_, _, _| {
            if let Some(v) = upgrade(&w) {
                v.0.canvas.begin_pan();
            }
        });
        let w = self.downgrade();
        drag.connect_drag_update(move |_, dx, dy| {
            if let Some(v) = upgrade(&w) {
                if v.0.canvas.is_zoomed() {
                    v.0.canvas.set_cursor_from_name(Some("grabbing"));
                    v.0.canvas.pan_to(dx, dy);
                }
            }
        });
        let w = self.downgrade();
        drag.connect_drag_end(move |_, _, _| {
            if let Some(v) = upgrade(&w) {
                v.0.canvas.set_cursor_from_name(None);
            }
        });
        i.canvas.add_controller(drag);

        for button in [gdk::BUTTON_PRIMARY, gdk::BUTTON_SECONDARY] {
            let lp = gtk::GestureLongPress::new();
            lp.set_button(button);
            let w = self.downgrade();
            lp.connect_pressed(move |g, x, y| {
                let Some(v) = upgrade(&w) else { return };
                if g.current_button() == gdk::BUTTON_PRIMARY {
                    v.0.sticky.set(ZoomMode::Actual);
                    v.0.canvas.actual_at(x, y);
                } else {
                    v.set_zoom_mode(ZoomMode::Fit);
                }
                v.update_hud();
                v.flash_hud();
            });
            i.canvas.add_controller(lp);
        }

        let click = gtk::GestureClick::new();
        click.set_button(0);
        let w = self.downgrade();
        click.connect_pressed(move |g, n, _, _| {
            let Some(v) = upgrade(&w) else { return };
            v.0.canvas.grab_focus();
            match g.current_button() {
                gdk::BUTTON_PRIMARY if n == 2 => v.close(),
                8 => v.step(-1),
                9 => v.step(1),
                _ => {}
            }
        });
        i.canvas.add_controller(click);
    }

    /// Path of the image on screen (for window-level actions).
    pub fn current_path(&self) -> Option<PathBuf> {
        self.current().map(|i| i.path())
    }

    pub fn forget(&self, path: &Path) {
        self.0.loader.forget(path);
    }
}

fn upgrade(w: &Weak<Inner>) -> Option<Viewer> {
    w.upgrade().map(Viewer)
}
