//! Waterfall (masonry) layout: fixed-width columns, cells keep their aspect
//! ratio and are placed in the currently shortest column. This is a
//! virtualised `GtkScrollable` widget over the same selection model as the
//! grid: only cells intersecting the viewport (plus overscan) exist, and
//! cell widgets are recycled.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use gtk::glib;
use gtk::graphene;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use super::{ThumbCell, ViewConfig};
use crate::model::ImageItem;

pub const GAP: f64 = 12.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub col: usize,
}

impl Rect {
    fn center_y(&self) -> f64 {
        self.y + self.h / 2.0
    }
    fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

/// Shortest-column-first packing. `aspects` are height/width ratios;
/// `extra` is the fixed label height added under every image.
/// Returns the rectangles, the content height and the column count.
pub fn pack(aspects: &[f64], width: f64, col_w: f64, gap: f64, extra: f64) -> (Vec<Rect>, f64, usize) {
    let ncols = (((width - gap) / (col_w + gap)).floor() as usize).max(1);
    let cw = ((width - gap * (ncols as f64 + 1.0)) / ncols as f64).max(16.0);
    let mut heights = vec![gap; ncols];
    let mut out = Vec::with_capacity(aspects.len());
    for &a in aspects {
        let (col, &y) =
            heights.iter().enumerate().min_by(|a, b| a.1.partial_cmp(b.1).unwrap().then(a.0.cmp(&b.0))).unwrap();
        let h = (cw * a).round() + extra;
        out.push(Rect { x: gap + col as f64 * (cw + gap), y, w: cw, h, col });
        heights[col] += h + gap;
    }
    let total = heights.into_iter().fold(0.0, f64::max);
    (out, total, ncols)
}

/// `gtk::ScrollablePolicy` lacks `Default`.
#[derive(Clone, Copy)]
pub struct Policy(pub gtk::ScrollablePolicy);
impl Default for Policy {
    fn default() -> Self {
        Policy(gtk::ScrollablePolicy::Minimum)
    }
}

mod imp {
    use super::*;
    use glib::subclass::Signal;
    use std::sync::OnceLock;

    #[derive(Default)]
    pub struct WaterfallView {
        pub selection: RefCell<Option<gtk::MultiSelection>>,
        pub cfg: RefCell<Option<Rc<ViewConfig>>>,
        pub rects: RefCell<Vec<Rect>>,
        pub aspects: RefCell<Vec<f64>>,
        pub content_height: Cell<f64>,
        pub layout_width: Cell<i32>,
        pub dirty: Cell<bool>,
        pub children: RefCell<HashMap<u32, ThumbCell>>,
        pub watched: RefCell<HashMap<u32, (ImageItem, glib::SignalHandlerId)>>,
        pub pool: RefCell<Vec<ThumbCell>>,
        pub hadj: RefCell<Option<gtk::Adjustment>>,
        pub vadj: RefCell<Option<gtk::Adjustment>>,
        pub vadj_handler: RefCell<Option<glib::SignalHandlerId>>,
        pub hpolicy: Cell<Policy>,
        pub vpolicy: Cell<Policy>,
        pub cursor: Cell<Option<u32>>,
        pub anchor: Cell<Option<u32>>,
        pub model_handlers: RefCell<Vec<glib::SignalHandlerId>>,
        pub pending_scroll: Cell<Option<u32>>,
        pub active: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for WaterfallView {
        const NAME: &'static str = "FlowlinWaterfallView";
        type Type = super::WaterfallView;
        type ParentType = gtk::Widget;
        type Interfaces = (gtk::Scrollable,);

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("waterfallview");
        }
    }

    impl ObjectImpl for WaterfallView {
        fn properties() -> &'static [glib::ParamSpec] {
            static PROPS: OnceLock<Vec<glib::ParamSpec>> = OnceLock::new();
            PROPS.get_or_init(|| {
                vec![
                    glib::ParamSpecOverride::for_interface::<gtk::Scrollable>("hadjustment"),
                    glib::ParamSpecOverride::for_interface::<gtk::Scrollable>("vadjustment"),
                    glib::ParamSpecOverride::for_interface::<gtk::Scrollable>("hscroll-policy"),
                    glib::ParamSpecOverride::for_interface::<gtk::Scrollable>("vscroll-policy"),
                ]
            })
        }

        fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            match pspec.name() {
                "hadjustment" => {
                    *self.hadj.borrow_mut() = value.get().unwrap();
                }
                "vadjustment" => {
                    let adj: Option<gtk::Adjustment> = value.get().unwrap();
                    if let (Some(old), Some(id)) = (self.vadj.borrow().as_ref(), self.vadj_handler.take()) {
                        old.disconnect(id);
                    }
                    if let Some(a) = &adj {
                        let w = self.obj().downgrade();
                        let id = a.connect_value_changed(move |_| {
                            if let Some(w) = w.upgrade() {
                                w.queue_allocate();
                            }
                        });
                        *self.vadj_handler.borrow_mut() = Some(id);
                    }
                    *self.vadj.borrow_mut() = adj;
                    self.obj().queue_allocate();
                }
                "hscroll-policy" => self.hpolicy.set(Policy(value.get().unwrap())),
                "vscroll-policy" => self.vpolicy.set(Policy(value.get().unwrap())),
                _ => unreachable!(),
            }
        }

        fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            match pspec.name() {
                "hadjustment" => self.hadj.borrow().to_value(),
                "vadjustment" => self.vadj.borrow().to_value(),
                "hscroll-policy" => self.hpolicy.get().0.to_value(),
                "vscroll-policy" => self.vpolicy.get().0.to_value(),
                _ => unreachable!(),
            }
        }

        fn signals() -> &'static [Signal] {
            static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| vec![Signal::builder("activate").param_types([u32::static_type()]).build()])
        }

        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.set_focusable(true);
            obj.set_overflow(gtk::Overflow::Hidden);
            obj.add_css_class("view");
            obj.setup_input();
        }

        fn dispose(&self) {
            let obj = self.obj();
            obj.reset_children();
            for c in self.pool.borrow_mut().drain(..) {
                c.unparent();
            }
        }
    }

    impl WidgetImpl for WaterfallView {
        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let size = self.cfg.borrow().as_ref().map(|c| c.size.get()).unwrap_or(200);
            match orientation {
                gtk::Orientation::Horizontal => (size + 2 * GAP as i32, size + 2 * GAP as i32, -1, -1),
                _ => (0, 0, -1, -1),
            }
        }

        fn size_allocate(&self, width: i32, height: i32, _baseline: i32) {
            self.obj().do_allocate(width, height);
        }
    }

    impl ScrollableImpl for WaterfallView {}
}

glib::wrapper! {
    pub struct WaterfallView(ObjectSubclass<imp::WaterfallView>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Scrollable;
}

impl WaterfallView {
    pub fn new(selection: &gtk::MultiSelection, cfg: Rc<ViewConfig>) -> Self {
        let v: Self = glib::Object::new();
        let imp = v.imp();
        *imp.cfg.borrow_mut() = Some(cfg);
        *imp.selection.borrow_mut() = Some(selection.clone());
        imp.dirty.set(true);
        let w = v.downgrade();
        let h1 = selection.connect_items_changed(move |_, _, _, _| {
            if let Some(v) = w.upgrade() {
                v.imp().cursor.set(None);
                v.imp().anchor.set(None);
                if v.imp().active.get() {
                    v.reset_children();
                    v.invalidate();
                }
            }
        });
        let w = v.downgrade();
        let h2 = selection.connect_selection_changed(move |_, _, _| {
            if let Some(v) = w.upgrade() {
                v.update_selection_classes();
            }
        });
        *imp.model_handlers.borrow_mut() = vec![h1, h2];
        v
    }

    /// Hidden waterfalls ignore model churn and release their cells.
    pub fn set_active(&self, active: bool) {
        if self.imp().active.replace(active) != active {
            self.reset_children();
            self.invalidate();
        }
    }

    fn selection(&self) -> gtk::MultiSelection {
        self.imp().selection.borrow().clone().unwrap()
    }

    fn cfg(&self) -> Rc<ViewConfig> {
        self.imp().cfg.borrow().clone().unwrap()
    }

    fn vadj(&self) -> Option<gtk::Adjustment> {
        self.imp().vadj.borrow().clone()
    }

    /// Layout must be recomputed (sizes, labels or aspect ratios changed).
    pub fn invalidate(&self) {
        self.imp().dirty.set(true);
        self.queue_allocate();
    }

    /// Size or label config changed.
    pub fn config_changed(&self) {
        self.reset_children();
        self.invalidate();
    }

    fn label_height(&self) -> f64 {
        let cfg = self.cfg();
        let mut h = 0.0;
        if cfg.show_names.get() {
            h += 22.0;
        }
        if cfg.show_folders.get() {
            h += 20.0;
        }
        h
    }

    fn relayout(&self, width: i32) {
        let imp = self.imp();
        let model = self.selection();
        let n = model.n_items();
        let mut aspects = Vec::with_capacity(n as usize);
        for i in 0..n {
            let a = model.item(i).and_downcast::<ImageItem>().map(|it| it.aspect()).unwrap_or(1.0);
            aspects.push(a);
        }
        // Keep the first visible cell anchored so relayouts do not jump.
        let anchor = self.vadj().and_then(|adj| {
            let top = adj.value();
            let rects = imp.rects.borrow();
            rects
                .iter()
                .enumerate()
                .filter(|(_, r)| r.y + r.h > top)
                .min_by(|a, b| a.1.y.partial_cmp(&b.1.y).unwrap())
                .map(|(i, r)| (i, r.y - top))
        });
        let (rects, total, _) = pack(&aspects, width as f64, self.cfg().size.get() as f64, GAP, self.label_height());
        imp.content_height.set(total);
        *imp.rects.borrow_mut() = rects;
        *imp.aspects.borrow_mut() = aspects;
        imp.layout_width.set(width);
        imp.dirty.set(false);
        if let (Some(adj), Some((i, off))) = (self.vadj(), anchor) {
            if let Some(r) = imp.rects.borrow().get(i) {
                if adj.value() > 0.0 {
                    adj.set_upper(total.max(adj.page_size()));
                    adj.set_value((r.y - off).max(0.0));
                }
            }
        }
    }

    fn do_allocate(&self, width: i32, height: i32) {
        let imp = self.imp();
        if !imp.active.get() {
            return;
        }
        if imp.dirty.get() || imp.layout_width.get() != width {
            if imp.layout_width.get() != width {
                self.reset_children();
            }
            self.relayout(width);
        }
        let total = imp.content_height.get();
        if let Some(h) = imp.hadj.borrow().as_ref() {
            h.configure(0.0, 0.0, width as f64, width as f64 * 0.1, width as f64 * 0.9, width as f64);
        }
        let Some(adj) = self.vadj() else { return };
        let upper = total.max(height as f64);
        let value = adj.value().clamp(0.0, (upper - height as f64).max(0.0));
        adj.configure(value, 0.0, upper, 64.0, height as f64 * 0.9, height as f64);
        if let Some(pos) = imp.pending_scroll.take() {
            self.scroll_to(pos);
        }
        self.sync_children(adj.value(), height as f64);
    }

    fn sync_children(&self, top: f64, height: f64) {
        let imp = self.imp();
        let over = height * 0.75;
        let (lo, hi) = (top - over, top + height + over);
        let rects = imp.rects.borrow().clone();
        let model = self.selection();
        let cfg = self.cfg();

        // Recycle cells that left the window.
        let stale: Vec<u32> = imp
            .children
            .borrow()
            .keys()
            .copied()
            .filter(|&i| rects.get(i as usize).is_none_or(|r| r.y + r.h < lo || r.y > hi))
            .collect();
        for i in stale {
            self.recycle(i);
        }

        let selected = model.selection();
        let cursor = imp.cursor.get();
        for (i, r) in rects.iter().enumerate() {
            if r.y + r.h < lo || r.y > hi {
                continue;
            }
            let i = i as u32;
            let existing = imp.children.borrow().get(&i).cloned();
            let cell = match existing {
                Some(c) => c,
                None => {
                    let Some(item) = model.item(i).and_downcast::<ImageItem>() else { continue };
                    let cell = imp.pool.borrow_mut().pop().unwrap_or_else(|| {
                        let c = ThumbCell::new(true);
                        c.set_parent(self);
                        c
                    });
                    cell.set_child_visible(true);
                    cfg.scale.set(self.scale_factor());
                    cell.bind(&item, &cfg);
                    self.watch(i, &item);
                    imp.children.borrow_mut().insert(i, cell.clone());
                    cell
                }
            };
            cell.set_width_override(r.w as i32);
            set_class(&cell, "selected", selected.contains(i));
            set_class(&cell, "cursor", cursor == Some(i));
            // Children must be measured before allocation.
            let _ = cell.measure(gtk::Orientation::Horizontal, -1);
            let (min_h, _, _, _) = cell.measure(gtk::Orientation::Vertical, r.w as i32);
            let t = gsk_translate(r.x, r.y - top);
            cell.allocate(r.w as i32, (r.h as i32).max(min_h), -1, Some(t));
        }
    }

    fn watch(&self, i: u32, item: &ImageItem) {
        let w = self.downgrade();
        let id = item.connect_changed(move |it| {
            let Some(v) = w.upgrade() else { return };
            let known = v.imp().aspects.borrow().get(i as usize).copied();
            if known.is_some_and(|a| (a - it.aspect()).abs() > 0.01) {
                v.invalidate();
            }
        });
        if let Some((old, oid)) = self.imp().watched.borrow_mut().insert(i, (item.clone(), id)) {
            old.disconnect(oid);
        }
    }

    fn recycle(&self, i: u32) {
        let imp = self.imp();
        if let Some(c) = imp.children.borrow_mut().remove(&i) {
            c.unbind();
            c.set_child_visible(false);
            imp.pool.borrow_mut().push(c);
        }
        if let Some((item, id)) = imp.watched.borrow_mut().remove(&i) {
            item.disconnect(id);
        }
    }

    fn reset_children(&self) {
        let keys: Vec<u32> = self.imp().children.borrow().keys().copied().collect();
        for k in keys {
            self.recycle(k);
        }
    }

    fn update_selection_classes(&self) {
        let sel = self.selection().selection();
        let cursor = self.imp().cursor.get();
        for (i, c) in self.imp().children.borrow().iter() {
            set_class(c, "selected", sel.contains(*i));
            set_class(c, "cursor", cursor == Some(*i));
        }
    }

    pub fn index_at(&self, x: f64, y: f64) -> Option<u32> {
        let top = self.vadj().map(|a| a.value()).unwrap_or(0.0);
        self.imp().rects.borrow().iter().position(|r| r.contains(x, y + top)).map(|i| i as u32)
    }

    /// Scroll so that `pos` is fully visible.
    pub fn scroll_to(&self, pos: u32) {
        let imp = self.imp();
        if imp.dirty.get() || imp.rects.borrow().is_empty() {
            imp.pending_scroll.set(Some(pos));
            self.queue_allocate();
            return;
        }
        let Some(r) = imp.rects.borrow().get(pos as usize).copied() else { return };
        let Some(adj) = self.vadj() else { return };
        let (top, page) = (adj.value(), adj.page_size());
        if r.y - GAP < top {
            adj.set_value((r.y - GAP).max(0.0));
        } else if r.y + r.h + GAP > top + page {
            adj.set_value(r.y + r.h + GAP - page);
        }
    }

    pub fn set_cursor(&self, pos: u32, select: bool, extend: bool) {
        let imp = self.imp();
        let sel = self.selection();
        imp.cursor.set(Some(pos));
        if select {
            if extend {
                let a = imp.anchor.get().unwrap_or(pos);
                let (lo, hi) = (a.min(pos), a.max(pos));
                sel.select_range(lo, hi - lo + 1, true);
            } else {
                sel.select_item(pos, true);
                imp.anchor.set(Some(pos));
            }
        }
        self.update_selection_classes();
        self.scroll_to(pos);
    }

    fn cursor_or_selected(&self) -> Option<u32> {
        let sel = self.selection().selection();
        self.imp().cursor.get().or_else(|| (!sel.is_empty()).then(|| sel.minimum()))
    }

    /// Visually nearest cell in a direction (waterfall-aware navigation).
    pub fn neighbour(&self, from: u32, dx: i32, dy: i32) -> Option<u32> {
        let rects = self.imp().rects.borrow();
        let r = *rects.get(from as usize)?;
        let ncols = rects.iter().map(|r| r.col).max().unwrap_or(0) + 1;
        let best = |pred: &dyn Fn(&Rect) -> bool, score: &dyn Fn(&Rect) -> f64| {
            rects
                .iter()
                .enumerate()
                .filter(|(i, c)| *i != from as usize && pred(c))
                .min_by(|a, b| score(a.1).partial_cmp(&score(b.1)).unwrap())
                .map(|(i, _)| i as u32)
        };
        if dx != 0 {
            let col = r.col as i32 + dx;
            if col < 0 || col >= ncols as i32 {
                return None;
            }
            best(&|c| c.col == col as usize, &|c| (c.center_y() - r.center_y()).abs())
        } else if dy < 0 {
            best(&|c| c.col == r.col && c.y < r.y, &|c| r.y - c.y)
        } else {
            best(&|c| c.col == r.col && c.y > r.y, &|c| c.y - r.y)
        }
    }

    fn page_target(&self, from: u32, down: bool) -> Option<u32> {
        let rects = self.imp().rects.borrow();
        let r = *rects.get(from as usize)?;
        let page = self.vadj().map(|a| a.page_size()).unwrap_or(600.0);
        let target = if down { r.center_y() + page } else { r.center_y() - page };
        rects
            .iter()
            .enumerate()
            .filter(|(_, c)| c.col == r.col)
            .min_by(|a, b| (a.1.center_y() - target).abs().partial_cmp(&(b.1.center_y() - target).abs()).unwrap())
            .map(|(i, _)| i as u32)
    }

    fn setup_input(&self) {
        let click = gtk::GestureClick::new();
        click.set_button(gtk::gdk::BUTTON_PRIMARY);
        let w = self.downgrade();
        click.connect_pressed(move |g, n, x, y| {
            let Some(v) = w.upgrade() else { return };
            v.grab_focus();
            let state = g.current_event_state();
            let ctrl = state.contains(gtk::gdk::ModifierType::CONTROL_MASK);
            let shift = state.contains(gtk::gdk::ModifierType::SHIFT_MASK);
            let sel = v.selection();
            match v.index_at(x, y) {
                Some(i) if n == 2 => {
                    v.emit_by_name::<()>("activate", &[&i]);
                }
                Some(i) => {
                    if ctrl {
                        if sel.is_selected(i) {
                            sel.unselect_item(i);
                        } else {
                            sel.select_item(i, false);
                        }
                        v.imp().cursor.set(Some(i));
                        v.imp().anchor.set(Some(i));
                        v.update_selection_classes();
                    } else {
                        v.set_cursor(i, true, shift);
                    }
                }
                None if !ctrl && !shift => {
                    sel.unselect_all();
                }
                None => {}
            }
        });
        self.add_controller(click);

        let keys = gtk::EventControllerKey::new();
        let w = self.downgrade();
        keys.connect_key_pressed(move |_, key, _, state| {
            use gtk::gdk::Key;
            let Some(v) = w.upgrade() else { return glib::Propagation::Proceed };
            let n = v.selection().n_items();
            if n == 0 {
                return glib::Propagation::Proceed;
            }
            let shift = state.contains(gtk::gdk::ModifierType::SHIFT_MASK);
            let ctrl = state.contains(gtk::gdk::ModifierType::CONTROL_MASK);
            let cur = v.cursor_or_selected();
            let target = match key {
                Key::Left | Key::KP_Left => cur.and_then(|c| v.neighbour(c, -1, 0)).or(Some(0)),
                Key::Right | Key::KP_Right => cur.and_then(|c| v.neighbour(c, 1, 0)).or(Some(0)),
                Key::Up | Key::KP_Up => cur.and_then(|c| v.neighbour(c, 0, -1)).or(Some(0)),
                Key::Down | Key::KP_Down => cur.and_then(|c| v.neighbour(c, 0, 1)).or(Some(0)),
                Key::Home | Key::KP_Home => Some(0),
                Key::End | Key::KP_End => Some(n - 1),
                Key::Page_Down | Key::KP_Page_Down => cur.and_then(|c| v.page_target(c, true)),
                Key::Page_Up | Key::KP_Page_Up => cur.and_then(|c| v.page_target(c, false)),
                Key::Return | Key::KP_Enter => {
                    if let Some(c) = cur {
                        v.emit_by_name::<()>("activate", &[&c]);
                    }
                    return glib::Propagation::Stop;
                }
                Key::a | Key::A if ctrl => {
                    v.selection().select_all();
                    return glib::Propagation::Stop;
                }
                _ => return glib::Propagation::Proceed,
            };
            if let Some(t) = target {
                v.set_cursor(t, true, shift);
            }
            glib::Propagation::Stop
        });
        self.add_controller(keys);
    }

    pub fn connect_activate<F: Fn(&Self, u32) + 'static>(&self, f: F) -> glib::SignalHandlerId {
        self.connect_closure("activate", false, glib::closure_local!(move |v: &WaterfallView, pos: u32| f(v, pos)))
    }
}

fn gsk_translate(x: f64, y: f64) -> gtk::gsk::Transform {
    gtk::gsk::Transform::new().translate(&graphene::Point::new(x as f32, y as f32))
}

fn set_class(w: &impl IsA<gtk::Widget>, class: &str, on: bool) {
    if on {
        w.add_css_class(class);
    } else {
        w.remove_css_class(class);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortest_column_first() {
        // width fits 3 columns of 100 with gap 10: 10+100+10+100+10+100+10 = 340
        let (r, total, ncols) = pack(&[1.0, 2.0, 0.5, 0.5, 1.0], 340.0, 100.0, 10.0, 0.0);
        assert_eq!(ncols, 3);
        assert_eq!(r[0].col, 0);
        assert_eq!(r[1].col, 1);
        assert_eq!(r[2].col, 2);
        // column heights after 3 items: c0=10+100+10=120, c1=10+200+10=220, c2=10+50+10=70
        assert_eq!(r[3].col, 2);
        assert_eq!(r[3].y, 70.0);
        // now c2=130 vs c0=120 → c0
        assert_eq!(r[4].col, 0);
        assert_eq!(r[4].y, 120.0);
        assert_eq!(total, 230.0);
    }

    #[test]
    fn columns_stretch_and_labels_add_height() {
        let (r, _, n) = pack(&[1.0], 355.0, 100.0, 10.0, 20.0);
        assert_eq!(n, 3);
        assert!((r[0].w - 105.0).abs() < 1e-9);
        assert_eq!(r[0].h, 105.0 + 20.0);
        let (_, _, n) = pack(&[1.0], 50.0, 100.0, 10.0, 0.0);
        assert_eq!(n, 1);
    }
}
