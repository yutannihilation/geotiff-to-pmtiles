use std::collections::HashMap;
use std::io::{Read, Seek};

use tiff::decoder::{Decoder, DecodingResult};

use super::cache::{ChunkData, ChunkKey};

/// Minimal chunk layout extracted from a tiff::decoder::Decoder.
/// Replaces the tiff-compio ChunkLayout with only the fields this crate needs.
#[derive(Debug, Clone)]
pub(crate) struct ChunkLayout {
    pub image_width: u32,
    pub image_height: u32,
    pub chunk_width: u32,
    pub chunk_height: u32,
    pub chunks_across: u32,
    pub chunks_down: u32,
    pub chunk_count: u32,
}

impl ChunkLayout {
    /// Build a ChunkLayout by querying a tiff Decoder for dimensions and chunk sizes.
    pub(crate) fn from_decoder<R: Read + Seek>(
        decoder: &mut Decoder<R>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let (image_width, image_height) = decoder.dimensions()?;
        let (chunk_width, chunk_height) = decoder.chunk_dimensions();

        let chunks_across = image_width.div_ceil(chunk_width);
        let chunks_down = image_height.div_ceil(chunk_height);
        let chunk_count = chunks_across * chunks_down;

        Ok(Self {
            image_width,
            image_height,
            chunk_width,
            chunk_height,
            chunks_across,
            chunks_down,
            chunk_count,
        })
    }

    /// Returns the actual pixel dimensions `(width, height)` of the chunk at index `idx`.
    ///
    /// Interior chunks have the full nominal size. Edge chunks on the right column
    /// or bottom row may be smaller if the image dimensions are not an exact multiple.
    pub(crate) fn chunk_data_dimensions(&self, idx: u32) -> (u32, u32) {
        let col = idx % self.chunks_across;
        let row = idx / self.chunks_across;

        let w = if (col + 1) * self.chunk_width > self.image_width {
            self.image_width - col * self.chunk_width
        } else {
            self.chunk_width
        };

        let h = if (row + 1) * self.chunk_height > self.image_height {
            self.image_height - row * self.chunk_height
        } else {
            self.chunk_height
        };

        (w, h)
    }
}

/// Convert a `tiff::decoder::DecodingResult` to `Vec<u8>`.
///
/// For U8 data this is a passthrough. For wider/signed/float types, a two-pass
/// min/max normalization maps the actual data range to [0, 255].
pub(crate) fn normalize_decoding_result(result: DecodingResult) -> Vec<u8> {
    match result {
        DecodingResult::U8(v) => v,
        DecodingResult::U16(v) => normalize_slice(&v, |x| *x as f64),
        DecodingResult::U32(v) => normalize_slice(&v, |x| *x as f64),
        DecodingResult::U64(v) => normalize_slice(&v, |x| *x as f64),
        DecodingResult::I8(v) => normalize_slice(&v, |x| *x as f64),
        DecodingResult::I16(v) => normalize_slice(&v, |x| *x as f64),
        DecodingResult::I32(v) => normalize_slice(&v, |x| *x as f64),
        DecodingResult::I64(v) => normalize_slice(&v, |x| *x as f64),
        DecodingResult::F32(v) => normalize_slice(&v, |x| *x as f64),
        DecodingResult::F16(v) => normalize_slice(&v, |x| f64::from(f32::from(*x))),
        DecodingResult::F64(v) => normalize_slice(&v, |x| *x),
    }
}

/// Two-pass min/max normalization to [0, 255].
fn normalize_slice<T>(data: &[T], to_f64: impl Fn(&T) -> f64) -> Vec<u8> {
    if data.is_empty() {
        return Vec::new();
    }

    // Pass 1: find actual data range
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    for v in data {
        let n = to_f64(v);
        if n.is_finite() {
            min = min.min(n);
            max = max.max(n);
        }
    }

    let range = if min.is_finite() && max.is_finite() {
        max - min
    } else {
        0.0
    };

    if range.abs() < f64::EPSILON {
        return vec![0; data.len()];
    }

    // Pass 2: normalize
    data.iter()
        .map(|v| {
            let t = ((to_f64(v) - min) / range).clamp(0.0, 1.0);
            (t * 255.0).round() as u8
        })
        .collect()
}

pub(super) struct SourceSampler {
    pub(super) reader: ChunkedTiffSampler,
}

/// Layout-only metadata for chunked TIFF sampling.
/// Does NOT hold a decoder — all I/O is done externally.
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
    pub(super) fn from_layout(layout: &ChunkLayout) -> Result<Self, Box<dyn std::error::Error>> {
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
        Ok(chunk_map
            .get(&key)
            .and_then(|chunk| pixel_from_chunk(chunk, lx, ly)))
    }
}
