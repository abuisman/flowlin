//! Byte-capped LRU of textures.

use std::collections::{BTreeMap, HashMap};
use std::hash::Hash;

use gtk::gdk;

pub struct Lru<K: Hash + Eq + Clone> {
    map: HashMap<K, (gdk::Texture, usize, u64)>,
    order: BTreeMap<u64, K>,
    tick: u64,
    bytes: usize,
    cap: usize,
}

impl<K: Hash + Eq + Clone> Lru<K> {
    pub fn new(cap: usize) -> Self {
        Self { map: HashMap::new(), order: BTreeMap::new(), tick: 0, bytes: 0, cap }
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn set_cap(&mut self, cap: usize) {
        self.cap = cap;
        self.evict();
    }

    pub fn get(&mut self, k: &K) -> Option<gdk::Texture> {
        self.tick += 1;
        let tick = self.tick;
        let (tex, _, t) = self.map.get_mut(k)?;
        self.order.remove(t);
        *t = tick;
        self.order.insert(tick, k.clone());
        Some(tex.clone())
    }

    pub fn insert(&mut self, k: K, tex: gdk::Texture, size: usize) {
        self.tick += 1;
        if let Some((_, s, t)) = self.map.remove(&k) {
            self.order.remove(&t);
            self.bytes -= s;
        }
        self.map.insert(k.clone(), (tex, size, self.tick));
        self.order.insert(self.tick, k);
        self.bytes += size;
        self.evict();
    }

    pub fn retain(&mut self, mut keep: impl FnMut(&K) -> bool) {
        let drop: Vec<K> = self.map.keys().filter(|k| !keep(k)).cloned().collect();
        for k in drop {
            if let Some((_, s, t)) = self.map.remove(&k) {
                self.order.remove(&t);
                self.bytes -= s;
            }
        }
    }

    fn evict(&mut self) {
        while self.bytes > self.cap {
            let Some((_, k)) = self.order.pop_first() else { break };
            if let Some((_, s, _)) = self.map.remove(&k) {
                self.bytes -= s;
            }
        }
    }
}
