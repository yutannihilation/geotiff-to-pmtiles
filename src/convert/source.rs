use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;

use tiff::decoder::Decoder;
use tiff::tags::Tag;

use crate::resample::decoding_result_to_u8;

use super::cache::{ChunkData, ChunkKey, GlobalChunkCache};

pub(super) enum SourceReader {
    Chunked(Box<ChunkedTiffSampler>),
    FullDeferred(Option<crate::resample::Raster>),
}

pub(super) struct SourceSampler {
    pub(super) path: PathBuf,
    pub(super) reader: SourceReader,
}

pub(super) struct ChunkedTiffSampler {
    decoder: Decoder<BufReader<File>>,
    width: usize,
    height: usize,
    samples: usize,
    chunk_w: usize,
    chunk_h: usize,
    chunks_across: usize,
    chunks_down: usize,
    chunk_count: usize,
}

impl ChunkedTiffSampler {
    pub(super) fn open(
        path: &std::path::Path,
        width: usize,
        height: usize,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let file = File::open(path)?;
        let reader = BufReader::new(file);
        let mut decoder = Decoder::new(reader)?;
        let samples = decoder
            .find_tag(Tag::SamplesPerPixel)?
            .map(|v| v.into_u16())
            .transpose()?
            .unwrap_or(1) as usize;
        let planar_config = decoder
            .find_tag(Tag::PlanarConfiguration)?
            .map(|v| v.into_u16())
            .transpose()?
            .unwrap_or(1);
        if planar_config == 2 {
            return Err("planar TIFF is not supported for chunked sampling".into());
        }

        let (cw_u32, ch_u32) = decoder.chunk_dimensions();
        let chunk_w = cw_u32 as usize;
        let chunk_h = ch_u32 as usize;
        if chunk_w == 0 || chunk_h == 0 {
            return Err("invalid chunk dimensions".into());
        }
        let chunks_across = width.div_ceil(chunk_w);
        let chunks_down = height.div_ceil(chunk_h);
        let chunk_count = match decoder.get_chunk_type() {
            tiff::decoder::ChunkType::Strip => decoder.strip_count()? as usize,
            tiff::decoder::ChunkType::Tile => decoder.tile_count()? as usize,
        };

        // Keep only geometry/layout metadata here; chunk payloads are decoded on demand.
        Ok(Self {
            decoder,
            width,
            height,
            samples,
            chunk_w,
            chunk_h,
            chunks_across,
            chunks_down,
            chunk_count,
        })
    }

    fn chunk_and_local(&self, xi: isize, yi: isize) -> Option<(u32, usize, usize)> {
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

    fn read_chunk_data(&mut self, chunk_idx: u32) -> Result<ChunkData, Box<dyn std::error::Error>> {
        // Decode one TIFF chunk/strip and normalize to the unified u8 storage used by samplers.
        let (cw_u32, ch_u32) = self.decoder.chunk_data_dimensions(chunk_idx);
        let chunk = self.decoder.read_chunk(chunk_idx)?;
        let data = decoding_result_to_u8(chunk);
        let cw = cw_u32 as usize;
        let ch = ch_u32 as usize;
        let stride = if cw == 0 || ch == 0 {
            self.samples.max(1)
        } else {
            (data.len() / (cw * ch)).max(1)
        };
        Ok(ChunkData {
            width: cw,
            height: ch,
            stride,
            data,
        })
    }

    fn prefetch_neighbors(
        &mut self,
        source_idx: usize,
        chunk_idx: u32,
        cache: &mut GlobalChunkCache,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // Spatial locality hint: scanline/bilinear sampling usually touches these
        // neighboring chunks immediately after the current one.
        let idx = chunk_idx as usize;
        let cx = idx % self.chunks_across;
        let cy = idx / self.chunks_across;
        if cy >= self.chunks_down {
            return Ok(());
        }

        let mut candidates = Vec::with_capacity(3);
        if cx + 1 < self.chunks_across {
            candidates.push((cy * self.chunks_across + (cx + 1)) as u32);
        }
        if cy + 1 < self.chunks_down {
            candidates.push((((cy + 1) * self.chunks_across) + cx) as u32);
            if cx + 1 < self.chunks_across {
                candidates.push((((cy + 1) * self.chunks_across) + (cx + 1)) as u32);
            }
        }

        for neighbor_idx in candidates {
            if neighbor_idx as usize >= self.chunk_count {
                continue;
            }
            let neighbor_key = ChunkKey {
                source_idx,
                chunk_idx: neighbor_idx,
            };
            if cache.contains(neighbor_key) {
                continue;
            }
            let chunk = self.read_chunk_data(neighbor_idx)?;
            cache.insert(neighbor_key, chunk);
        }
        Ok(())
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
    pub(super) fn sample_pixel_opt(
        &mut self,
        source_idx: usize,
        xi: isize,
        yi: isize,
        cache: &mut GlobalChunkCache,
    ) -> Result<Option<[u8; 4]>, Box<dyn std::error::Error>> {
        match &mut self.reader {
            SourceReader::Chunked(s) => {
                let Some((chunk_idx, lx, ly)) = s.chunk_and_local(xi, yi) else {
                    return Ok(None);
                };
                let key = ChunkKey {
                    source_idx,
                    chunk_idx,
                };
                // Decode only when missing in cache; neighboring pixels tend to hit.
                if !cache.contains(key) {
                    let chunk = s.read_chunk_data(chunk_idx)?;
                    cache.insert(key, chunk);
                    // Warm likely next chunks (right, down, diagonal) to reduce read stalls.
                    s.prefetch_neighbors(source_idx, chunk_idx, cache)?;
                }
                // Return RGBA from cached decoded chunk bytes.
                Ok(cache
                    .get(key)
                    .and_then(|chunk| pixel_from_chunk(chunk, lx, ly)))
            }
            SourceReader::FullDeferred(raster_opt) => {
                // Fallback path for TIFF layouts that are not chunk-sampled: lazily load full
                // raster only when this source is first touched.
                if raster_opt.is_none() {
                    *raster_opt = Some(crate::resample::load_raster(self.path.as_path())?);
                }
                Ok(raster_opt
                    .as_ref()
                    .and_then(|raster| sample_pixel_raster_opt(raster, xi, yi)))
            }
        }
    }

    pub(super) fn release_if_inactive(&mut self, active: bool) {
        if active {
            return;
        }
        match &mut self.reader {
            SourceReader::Chunked(_) => {}
            SourceReader::FullDeferred(r) => *r = None,
        }
    }
}

fn sample_pixel_raster_opt(
    raster: &crate::resample::Raster,
    xi: isize,
    yi: isize,
) -> Option<[u8; 4]> {
    // Shared pixel extraction helper for deferred full-raster fallback sampling.
    if xi < 0 || yi < 0 || xi >= raster.width as isize || yi >= raster.height as isize {
        return None;
    }
    let xi = xi as usize;
    let yi = yi as usize;
    let pixel_index = yi * raster.width + xi;
    let base = pixel_index.saturating_mul(raster.stride);
    if base >= raster.data.len() {
        return None;
    }
    let out = match raster.stride {
        0 => [0, 0, 0, 0],
        1 => {
            let g = raster.data[base];
            [g, g, g, 255]
        }
        2 => {
            let g = raster.data[base];
            let a = *raster.data.get(base + 1).unwrap_or(&255);
            [g, g, g, a]
        }
        _ => {
            let r = raster.data[base];
            let g = *raster.data.get(base + 1).unwrap_or(&r);
            let b = *raster.data.get(base + 2).unwrap_or(&r);
            let a = if raster.stride >= 4 {
                *raster.data.get(base + 3).unwrap_or(&255)
            } else {
                255
            };
            [r, g, b, a]
        }
    };
    Some(out)
}
