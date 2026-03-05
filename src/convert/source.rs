use std::collections::HashMap;

use super::cache::{ChunkData, ChunkKey};

pub(super) struct SourceSampler {
    pub(super) reader: ChunkedTiffSampler,
}

/// Layout-only metadata for chunked TIFF sampling.
/// Does NOT hold a decoder — all I/O is done externally via compio.
pub(super) struct ChunkedTiffSampler {
    pub(super) width: usize,
    pub(super) height: usize,
    pub(super) chunk_w: usize,
    pub(super) chunk_h: usize,
    pub(super) chunks_across: usize,
    pub(super) chunk_count: usize,
}

impl ChunkedTiffSampler {
    /// Build a sampler from pre-computed layout metadata (no file I/O).
    pub(super) fn from_layout(
        layout: &tiff_compio::ChunkLayout,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let chunk_w = layout.chunk_width as usize;
        let chunk_h = layout.chunk_height as usize;
        if chunk_w == 0 || chunk_h == 0 {
            return Err("invalid chunk dimensions".into());
        }

        Ok(Self {
            width: layout.image_width as usize,
            height: layout.image_height as usize,
            chunk_w,
            chunk_h,
            chunks_across: layout.chunks_across as usize,
            chunk_count: layout.chunk_count as usize,
        })
    }

    pub(super) fn chunk_and_local(&self, xi: isize, yi: isize) -> Option<(u32, usize, usize)> {
        // Map absolute pixel coordinate -> (chunk index, local x/y in chunk).
        if xi < 0 || yi < 0 || xi >= self.width as isize || yi >= self.height as isize {
            return None;
        }
        let xi = xi as usize;
        let yi = yi as usize;
        let cx = xi / self.chunk_w;
        let cy = yi / self.chunk_h;
        let chunk_idx = (cy * self.chunks_across + cx) as u32;
        if chunk_idx as usize >= self.chunk_count {
            return None;
        }
        let x0 = cx * self.chunk_w;
        let y0 = cy * self.chunk_h;
        Some((chunk_idx, xi.saturating_sub(x0), yi.saturating_sub(y0)))
    }
}

fn pixel_from_chunk(chunk: &ChunkData, lx: usize, ly: usize) -> Option<[u8; 4]> {
    if lx >= chunk.width || ly >= chunk.height {
        return None;
    }
    let pixel_index = ly * chunk.width + lx;
    let base = pixel_index.saturating_mul(chunk.stride);
    if base >= chunk.data.len() {
        return None;
    }
    let out = match chunk.stride {
        0 => [0, 0, 0, 0],
        1 => {
            let g = chunk.data[base];
            [g, g, g, 255]
        }
        2 => {
            let g = chunk.data[base];
            let a = *chunk.data.get(base + 1).unwrap_or(&255);
            [g, g, g, a]
        }
        _ => {
            let r = chunk.data[base];
            let g = *chunk.data.get(base + 1).unwrap_or(&r);
            let b = *chunk.data.get(base + 2).unwrap_or(&r);
            let a = if chunk.stride >= 4 {
                *chunk.data.get(base + 3).unwrap_or(&255)
            } else {
                255
            };
            [r, g, b, a]
        }
    };
    Some(out)
}

impl SourceSampler {
    /// Sample a pixel using pre-loaded chunk data from the batch map.
    pub(super) fn sample_pixel_opt(
        &mut self,
        source_idx: usize,
        xi: isize,
        yi: isize,
        chunk_map: &HashMap<ChunkKey, ChunkData>,
    ) -> Result<Option<[u8; 4]>, Box<dyn std::error::Error>> {
        let Some((chunk_idx, lx, ly)) = self.reader.chunk_and_local(xi, yi) else {
            return Ok(None);
        };
        let key = ChunkKey {
            source_idx,
            chunk_idx,
        };
        // Look up from pre-loaded chunk map (pure CPU, no I/O)
        Ok(chunk_map
            .get(&key)
            .and_then(|chunk| pixel_from_chunk(chunk, lx, ly)))
    }
}
