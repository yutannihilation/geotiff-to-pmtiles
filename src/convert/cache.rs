use std::collections::{HashMap, VecDeque};

pub(super) struct ChunkData {
    pub(super) width: usize,
    pub(super) height: usize,
    pub(super) stride: usize,
    pub(super) data: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ChunkKey {
    pub(super) source_idx: usize,
    pub(super) chunk_idx: u32,
}

pub(super) struct GlobalChunkCache {
    max_bytes: usize,
    used_bytes: usize,
    order: VecDeque<ChunkKey>,
    map: HashMap<ChunkKey, ChunkData>,
}

impl GlobalChunkCache {
    pub(super) fn new(max_bytes: usize) -> Self {
        Self {
            max_bytes,
            used_bytes: 0,
            order: VecDeque::new(),
            map: HashMap::new(),
        }
    }

    pub(super) fn get(&mut self, key: ChunkKey) -> Option<&ChunkData> {
        if self.map.contains_key(&key) {
            self.touch(key);
        }
        self.map.get(&key)
    }

    pub(super) fn insert(&mut self, key: ChunkKey, value: ChunkData) {
        let value_bytes = value.data.len();
        if self.map.contains_key(&key) {
            self.order.retain(|k| *k != key);
            if let Some(old) = self.map.remove(&key) {
                self.used_bytes = self.used_bytes.saturating_sub(old.data.len());
            }
        }
        // LRU eviction by total bytes, not item count.
        while self.used_bytes + value_bytes > self.max_bytes {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(old) = self.map.remove(&oldest) {
                self.used_bytes = self.used_bytes.saturating_sub(old.data.len());
            }
        }
        self.used_bytes += value_bytes;
        self.map.insert(key, value);
        self.order.push_back(key);
    }

    fn touch(&mut self, key: ChunkKey) {
        self.order.retain(|k| *k != key);
        self.order.push_back(key);
    }
}
