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
