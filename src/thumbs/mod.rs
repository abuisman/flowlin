//! Thumbnail pipeline. Thumbnails are decoded on a bounded rayon pool and
//! kept **only in memory** (a byte-capped LRU); nothing is ever written to
//! disk. Requests are LIFO so the most recently bound (visible) cells win,
//! cancelled when cells scroll away, and deduplicated per file.

mod lru;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

use crate::decode;
use crate::model::{ImageItem, ThumbState};
use lru::Lru;

/// Thumbnail resolutions we decode at (px on the longest side).
pub const TIERS: [i32; 4] = [128, 256, 512, 1024];

pub fn tier_for(px: i32) -> i32 {
    TIERS.into_iter().find(|t| *t >= px).unwrap_or(1024)
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
struct Key {
    path: PathBuf,
    mtime: i64,
    tier: i32,
}

type JobResult = (Key, Result<decode::Thumb, String>);

struct Inner {
    pool: rayon::ThreadPool,
    tx: async_channel::Sender<JobResult>,
    queue: RefCell<Vec<Key>>,
    waiters: RefCell<HashMap<Key, Vec<glib::WeakRef<ImageItem>>>>,
    in_flight: RefCell<HashSet<Key>>,
    max_in_flight: usize,
    cache: RefCell<Lru<Key>>,
    failed: RefCell<HashSet<(PathBuf, i64)>>,
    decoded: Cell<u64>,
}

#[derive(Clone)]
pub struct ThumbService(Rc<Inner>);

thread_local! {
    static SERVICE: ThumbService = ThumbService::new();
}

impl ThumbService {
    /// The shared service (main thread).
    pub fn get() -> ThumbService {
        SERVICE.with(|s| s.clone())
    }

    fn new() -> Self {
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).saturating_sub(1).clamp(2, 12);
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .thread_name(|i| format!("flowlin-thumb-{i}"))
            .build()
            .expect("thumbnail pool");
        let (tx, rx) = async_channel::unbounded::<JobResult>();
        let mb = crate::settings::settings().uint("thumbnail-memory") as usize;
        let inner = Rc::new(Inner {
            pool,
            tx,
            queue: Default::default(),
            waiters: Default::default(),
            in_flight: Default::default(),
            max_in_flight: threads * 2,
            cache: RefCell::new(Lru::new(mb * 1024 * 1024)),
            failed: Default::default(),
            decoded: Cell::new(0),
        });
        let weak = Rc::downgrade(&inner);
        glib::spawn_future_local(async move {
            while let Ok((key, res)) = rx.recv().await {
                let Some(inner) = weak.upgrade() else { break };
                ThumbService(inner).finish(key, res);
            }
        });
        tracing::debug!("thumbnail pool: {threads} threads, {mb} MB memory cache");
        ThumbService(inner)
    }

    pub fn set_memory_cap(&self, mb: u32) {
        self.0.cache.borrow_mut().set_cap(mb as usize * 1024 * 1024);
    }

    fn key(item: &ImageItem, px: i32) -> Key {
        Key { path: item.path(), mtime: item.mtime(), tier: tier_for(px) }
    }

    /// Ask for `item`'s thumbnail at `px` device pixels. The texture arrives
    /// via `item.set_texture`.
    pub fn request(&self, item: &ImageItem, px: i32) {
        let key = Self::key(item, px);
        if self.0.failed.borrow().contains(&(key.path.clone(), key.mtime)) {
            item.set_thumb_state(ThumbState::Failed);
            return;
        }
        if let Some(tex) = self.0.cache.borrow_mut().get(&key) {
            item.set_thumb_state(ThumbState::Ready);
            item.set_texture(Some(tex));
            return;
        }
        // A lower tier from the cache is a better placeholder than nothing.
        if item.texture().is_none() {
            for t in TIERS {
                if t == key.tier {
                    continue;
                }
                let k = Key { tier: t, ..key.clone() };
                if let Some(tex) = self.0.cache.borrow_mut().get(&k) {
                    item.set_texture(Some(tex));
                    break;
                }
            }
        }
        item.set_thumb_state(ThumbState::Pending);
        {
            let mut waiters = self.0.waiters.borrow_mut();
            let list = waiters.entry(key.clone()).or_default();
            if !list.iter().any(|w| w.upgrade().as_ref() == Some(item)) {
                list.push(item.downgrade());
            }
        }
        if !self.0.in_flight.borrow().contains(&key) {
            let mut q = self.0.queue.borrow_mut();
            if let Some(i) = q.iter().position(|k| *k == key) {
                q.remove(i);
            }
            q.push(key);
        }
        self.pump();
    }

    /// The cell showing `item` went away: drop queued work and, if nothing
    /// else shows the item, its texture reference (the LRU keeps a copy).
    pub fn release(&self, item: &ImageItem) {
        if item.bound() > 0 {
            return;
        }
        let mut waiters = self.0.waiters.borrow_mut();
        let mut emptied = Vec::new();
        for (k, list) in waiters.iter_mut() {
            list.retain(|w| w.upgrade().is_some_and(|i| i != *item));
            if list.is_empty() {
                emptied.push(k.clone());
            }
        }
        for k in &emptied {
            waiters.remove(k);
        }
        drop(waiters);
        if !emptied.is_empty() {
            self.0.queue.borrow_mut().retain(|k| !emptied.contains(k));
        }
        if item.thumb_state() == ThumbState::Pending {
            item.set_thumb_state(ThumbState::None);
        }
        item.set_texture(None);
    }

    /// Forget cached thumbnails for a file that changed on disk.
    pub fn invalidate(&self, path: &std::path::Path) {
        self.0.cache.borrow_mut().retain(|k| k.path != path);
        self.0.failed.borrow_mut().retain(|(p, _)| p != path);
    }

    fn pump(&self) {
        loop {
            if self.0.in_flight.borrow().len() >= self.0.max_in_flight {
                return;
            }
            let Some(key) = self.0.queue.borrow_mut().pop() else { return };
            self.0.in_flight.borrow_mut().insert(key.clone());
            let tx = self.0.tx.clone();
            self.0.pool.spawn(move || {
                let res = std::panic::catch_unwind(|| decode::thumbnail(&key.path, key.tier))
                    .unwrap_or_else(|_| Err("decoder panicked".into()));
                let _ = tx.send_blocking((key, res));
            });
        }
    }

    fn finish(&self, key: Key, res: Result<decode::Thumb, String>) {
        self.0.in_flight.borrow_mut().remove(&key);
        let waiters = self.0.waiters.borrow_mut().remove(&key).unwrap_or_default();
        match res {
            Ok(thumb) => {
                self.0.decoded.set(self.0.decoded.get() + 1);
                let tex = thumb.image.texture();
                self.0.cache.borrow_mut().insert(key, tex.clone(), thumb.image.byte_size());
                for w in waiters {
                    if let Some(item) = w.upgrade() {
                        item.set_dimensions(thumb.width, thumb.height);
                        item.set_thumb_state(ThumbState::Ready);
                        if item.bound() > 0 {
                            item.set_texture(Some(tex.clone()));
                        }
                    }
                }
            }
            Err(e) => {
                tracing::debug!("thumbnail failed for {}: {e}", key.path.display());
                self.0.failed.borrow_mut().insert((key.path.clone(), key.mtime));
                for w in waiters {
                    if let Some(item) = w.upgrade() {
                        item.set_thumb_state(ThumbState::Failed);
                    }
                }
            }
        }
        self.pump();
    }

    /// Number of thumbnails decoded so far (bench / diagnostics).
    pub fn decoded_count(&self) -> u64 {
        self.0.decoded.get()
    }

    pub fn cache_bytes(&self) -> usize {
        self.0.cache.borrow().bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiers() {
        assert_eq!(tier_for(96), 128);
        assert_eq!(tier_for(128), 128);
        assert_eq!(tier_for(208), 256);
        assert_eq!(tier_for(416), 512);
        assert_eq!(tier_for(4000), 1024);
    }
}
