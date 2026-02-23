use std::collections::HashMap;

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
    // Oldest key (LRU head).
    head: Option<ChunkKey>,
    // Most recent key (LRU tail).
    tail: Option<ChunkKey>,
    map: HashMap<ChunkKey, CacheNode>,
}

struct CacheNode {
    chunk: ChunkData,
    prev: Option<ChunkKey>,
    next: Option<ChunkKey>,
}

impl GlobalChunkCache {
    pub(super) fn new(max_bytes: usize) -> Self {
        Self {
            max_bytes,
            used_bytes: 0,
            head: None,
            tail: None,
            map: HashMap::new(),
        }
    }

    pub(super) fn get(&mut self, key: ChunkKey) -> Option<&ChunkData> {
        if self.map.contains_key(&key) {
            self.touch(key);
        }
        self.map.get(&key).map(|node| &node.chunk)
    }

    pub(super) fn contains(&self, key: ChunkKey) -> bool {
        // Presence check without LRU touch, useful for prefetch decisions.
        self.map.contains_key(&key)
    }

    pub(super) fn insert(&mut self, key: ChunkKey, value: ChunkData) {
        let value_bytes = value.data.len();
        if self.map.contains_key(&key) {
            self.remove(key);
        }
        // LRU eviction by total bytes, not item count.
        while self.used_bytes + value_bytes > self.max_bytes {
            let Some(oldest) = self.head else {
                break;
            };
            self.remove(oldest);
        }
        let inserted = self.map.insert(
            key,
            CacheNode {
                chunk: value,
                prev: self.tail,
                next: None,
            },
        );
        debug_assert!(inserted.is_none());
        self.used_bytes += value_bytes;

        if let Some(tail_key) = self.tail {
            if let Some(tail_node) = self.map.get_mut(&tail_key) {
                tail_node.next = Some(key);
            }
        } else {
            self.head = Some(key);
        }
        self.tail = Some(key);
    }

    fn touch(&mut self, key: ChunkKey) {
        if self.tail == Some(key) {
            return;
        }
        self.unlink(key);
        self.push_back(key);
    }

    fn remove(&mut self, key: ChunkKey) {
        let old_len = self
            .map
            .get(&key)
            .map(|node| node.chunk.data.len())
            .unwrap_or(0);
        self.unlink(key);
        let removed = self.map.remove(&key);
        if removed.is_some() {
            self.used_bytes = self.used_bytes.saturating_sub(old_len);
        }
    }

    fn unlink(&mut self, key: ChunkKey) {
        let (prev, next) = if let Some(node) = self.map.get(&key) {
            (node.prev, node.next)
        } else {
            return;
        };

        match prev {
            Some(prev_key) => {
                if let Some(prev_node) = self.map.get_mut(&prev_key) {
                    prev_node.next = next;
                }
            }
            None => {
                self.head = next;
            }
        }

        match next {
            Some(next_key) => {
                if let Some(next_node) = self.map.get_mut(&next_key) {
                    next_node.prev = prev;
                }
            }
            None => {
                self.tail = prev;
            }
        }

        if let Some(node) = self.map.get_mut(&key) {
            node.prev = None;
            node.next = None;
        }
    }

    fn push_back(&mut self, key: ChunkKey) {
        let old_tail = self.tail;
        if let Some(node) = self.map.get_mut(&key) {
            node.prev = old_tail;
            node.next = None;
        } else {
            return;
        }

        if let Some(tail_key) = old_tail {
            if let Some(tail_node) = self.map.get_mut(&tail_key) {
                tail_node.next = Some(key);
            }
        } else {
            self.head = Some(key);
        }

        self.tail = Some(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(source_idx: usize, chunk_idx: u32) -> ChunkKey {
        ChunkKey {
            source_idx,
            chunk_idx,
        }
    }

    fn chunk(bytes: usize) -> ChunkData {
        ChunkData {
            width: 1,
            height: 1,
            stride: bytes.max(1),
            data: vec![0; bytes],
        }
    }

    #[test]
    fn evicts_oldest_when_budget_exceeded() {
        let mut cache = GlobalChunkCache::new(5);
        let k1 = key(0, 1);
        let k2 = key(0, 2);

        cache.insert(k1, chunk(3));
        cache.insert(k2, chunk(3));

        assert!(!cache.contains(k1));
        assert!(cache.contains(k2));
    }

    #[test]
    fn get_touches_entry_for_lru_order() {
        let mut cache = GlobalChunkCache::new(6);
        let k1 = key(0, 1);
        let k2 = key(0, 2);
        let k3 = key(0, 3);

        cache.insert(k1, chunk(3));
        cache.insert(k2, chunk(3));
        let _ = cache.get(k1);
        cache.insert(k3, chunk(3));

        assert!(cache.contains(k1));
        assert!(!cache.contains(k2));
        assert!(cache.contains(k3));
    }

    #[test]
    fn replacing_existing_key_updates_size() {
        let mut cache = GlobalChunkCache::new(10);
        let k = key(1, 1);
        cache.insert(k, chunk(4));
        assert_eq!(cache.used_bytes, 4);

        cache.insert(k, chunk(2));
        assert_eq!(cache.used_bytes, 2);
        assert_eq!(cache.get(k).map(|c| c.data.len()), Some(2));
    }

    #[test]
    fn oversized_item_is_kept_when_cache_empty() {
        let mut cache = GlobalChunkCache::new(2);
        let k = key(9, 9);
        cache.insert(k, chunk(5));
        assert!(cache.contains(k));
        assert_eq!(cache.used_bytes, 5);
    }
}
