use std::collections::HashMap;
use std::sync::Arc;

/// Decoded chunk pixel data, normalized to u8.
pub(crate) struct ChunkData {
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) stride: usize,
    pub(crate) data: Vec<u8>,
}

/// Identifies a specific chunk within a specific source file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ChunkKey {
    pub(crate) source_idx: usize,
    pub(crate) chunk_idx: u32,
}

struct CacheEntry {
    chunk: Arc<ChunkData>,
    last_used: u64,
}

/// A deliberately small byte-bounded LRU for decoded TIFF chunks.
///
/// Looking up the least-recently-used entry is O(n). That keeps this preview
/// cache dependency-free and is acceptable for the expected cache sizes.
pub(crate) struct ChunkLruCache {
    entries: HashMap<ChunkKey, CacheEntry>,
    capacity_bytes: usize,
    used_bytes: usize,
    clock: u64,
}

impl ChunkLruCache {
    pub(crate) fn new(capacity_bytes: usize) -> Self {
        Self {
            entries: HashMap::new(),
            capacity_bytes,
            used_bytes: 0,
            clock: 0,
        }
    }

    pub(crate) fn get(&mut self, key: &ChunkKey) -> Option<Arc<ChunkData>> {
        self.clock = self.clock.wrapping_add(1);
        let entry = self.entries.get_mut(key)?;
        entry.last_used = self.clock;
        Some(Arc::clone(&entry.chunk))
    }

    pub(crate) fn insert(&mut self, key: ChunkKey, chunk: Arc<ChunkData>) {
        let chunk_bytes = chunk.data.len();
        if self.capacity_bytes == 0 || chunk_bytes > self.capacity_bytes {
            return;
        }

        self.clock = self.clock.wrapping_add(1);
        if let Some(previous) = self.entries.remove(&key) {
            self.used_bytes = self.used_bytes.saturating_sub(previous.chunk.data.len());
        }
        self.used_bytes = self.used_bytes.saturating_add(chunk_bytes);
        self.entries.insert(
            key,
            CacheEntry {
                chunk,
                last_used: self.clock,
            },
        );

        while self.used_bytes > self.capacity_bytes {
            let Some(lru_key) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| *key)
            else {
                break;
            };
            if let Some(removed) = self.entries.remove(&lru_key) {
                self.used_bytes = self.used_bytes.saturating_sub(removed.chunk.data.len());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(size: usize) -> Arc<ChunkData> {
        Arc::new(ChunkData {
            width: size,
            height: 1,
            stride: 1,
            data: vec![0; size],
        })
    }

    #[test]
    fn evicts_the_least_recently_used_chunk_by_byte_budget() {
        let first = ChunkKey {
            source_idx: 0,
            chunk_idx: 0,
        };
        let second = ChunkKey {
            source_idx: 0,
            chunk_idx: 1,
        };
        let third = ChunkKey {
            source_idx: 0,
            chunk_idx: 2,
        };
        let mut cache = ChunkLruCache::new(8);
        cache.insert(first, chunk(4));
        cache.insert(second, chunk(4));
        assert!(cache.get(&first).is_some());

        cache.insert(third, chunk(4));

        assert!(cache.get(&first).is_some());
        assert!(cache.get(&second).is_none());
        assert!(cache.get(&third).is_some());
    }
}
