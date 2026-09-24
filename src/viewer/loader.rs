//! Background full-size decoding with a small LRU of decoded images
//! (≈5 images or 512 MB, whichever is smaller) so stepping is instant.

use std::cell::RefCell;
use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::gdk;
use gtk::glib;

use crate::decode::{self, Full};

const MAX_ENTRIES: usize = 5;
const MAX_BYTES: usize = 512 * 1024 * 1024;

pub struct Loaded {
    pub frames: Vec<(gdk::Texture, u32)>,
    pub width: u32,
    pub height: u32,
    bytes: usize,
}

pub type LoadResult = Result<Rc<Loaded>, String>;

type LoadedHandler = Box<dyn Fn(&Path)>;

struct Inner {
    pool: rayon::ThreadPool,
    tx: async_channel::Sender<(PathBuf, i64, Result<Full, String>)>,
    cache: RefCell<VecDeque<(PathBuf, i64, LoadResult)>>,
    in_flight: RefCell<HashSet<(PathBuf, i64)>>,
    on_loaded: RefCell<Option<LoadedHandler>>,
}

#[derive(Clone)]
pub struct Loader(Rc<Inner>);

impl Loader {
    pub fn new() -> Self {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(2)
            .thread_name(|i| format!("flowlin-view-{i}"))
            .build()
            .expect("viewer pool");
        let (tx, rx) = async_channel::unbounded();
        let inner = Rc::new(Inner {
            pool,
            tx,
            cache: Default::default(),
            in_flight: Default::default(),
            on_loaded: Default::default(),
        });
        let weak = Rc::downgrade(&inner);
        glib::spawn_future_local(async move {
            while let Ok((path, mtime, res)) = rx.recv().await {
                let Some(inner) = weak.upgrade() else { break };
                Loader(inner).finish(path, mtime, res);
            }
        });
        Loader(inner)
    }

    pub fn connect_loaded(&self, f: impl Fn(&Path) + 'static) {
        *self.0.on_loaded.borrow_mut() = Some(Box::new(f));
    }

    /// Cached result for `path`, refreshing its LRU position.
    pub fn get(&self, path: &Path, mtime: i64) -> Option<LoadResult> {
        let mut c = self.0.cache.borrow_mut();
        let i = c.iter().position(|(p, m, _)| p == path && *m == mtime)?;
        let e = c.remove(i).unwrap();
        let r = e.2.clone();
        c.push_back(e);
        Some(r)
    }

    pub fn request(&self, path: &Path, mtime: i64) {
        if self.0.cache.borrow().iter().any(|(p, m, _)| p == path && *m == mtime) {
            return;
        }
        if !self.0.in_flight.borrow_mut().insert((path.to_path_buf(), mtime)) {
            return;
        }
        let tx = self.0.tx.clone();
        let path = path.to_path_buf();
        self.0.pool.spawn(move || {
            let res = std::panic::catch_unwind(|| decode::load_full(&path))
                .unwrap_or_else(|_| Err("decoder panicked".into()));
            let _ = tx.send_blocking((path, mtime, res));
        });
    }

    pub fn forget(&self, path: &Path) {
        self.0.cache.borrow_mut().retain(|(p, _, _)| p != path);
    }

    fn finish(&self, path: PathBuf, mtime: i64, res: Result<Full, String>) {
        self.0.in_flight.borrow_mut().remove(&(path.clone(), mtime));
        let loaded: LoadResult = res.map(|full| {
            let bytes = full.byte_size();
            Rc::new(match full {
                Full::Static { image, width, height } => {
                    Loaded { frames: vec![(image.texture(), 0)], width, height, bytes }
                }
                Full::Animated { frames, width, height } => {
                    Loaded { frames: frames.into_iter().map(|(f, d)| (f.texture(), d)).collect(), width, height, bytes }
                }
            })
        });
        if let Err(e) = &loaded {
            tracing::info!("cannot display {}: {e}", path.display());
        }
        {
            let mut c = self.0.cache.borrow_mut();
            c.retain(|(p, _, _)| *p != path);
            c.push_back((path.clone(), mtime, loaded));
            let size = |c: &VecDeque<(PathBuf, i64, LoadResult)>| -> usize {
                c.iter().map(|(_, _, r)| r.as_ref().map(|l| l.bytes).unwrap_or(0)).sum()
            };
            while c.len() > MAX_ENTRIES || (c.len() > 1 && size(&c) > MAX_BYTES) {
                c.pop_front();
            }
        }
        if let Some(f) = self.0.on_loaded.borrow().as_ref() {
            f(&path);
        }
    }
}

impl Default for Loader {
    fn default() -> Self {
        Self::new()
    }
}
