use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::BufReader;
use std::sync::{Arc, Mutex};

use tiff::decoder::Decoder;

use crate::cli::{PngCompression, TileFormat};
use crate::resample::{NoDataSpec, Pt, TILE_SIZE, parse_nodata, source_corners_merc_georef};

use super::cache::{ChunkData, ChunkKey, ChunkLruCache};
use super::render::render_tile_chunked;
use super::{ConvertOptions, SourceSpec, compute_chunk_requirements, make_samplers, open_sources};

struct PreviewIo {
    decoders: Vec<Decoder<BufReader<File>>>,
    cache: ChunkLruCache,
}

/// Reusable renderer for serving arbitrary XYZ tiles from fixed local sources.
pub(crate) struct PreviewRenderer {
    source_specs: Vec<SourceSpec>,
    source_bounds: Vec<(f64, f64, f64, f64)>,
    layouts: Vec<super::source::ChunkLayout>,
    io: Mutex<PreviewIo>,
    nodata: Option<NoDataSpec>,
    resampling: crate::cli::Resampling,
    tile_format: TileFormat,
    avif_encoder: Option<ravif::Encoder<'static>>,
    png_compression: PngCompression,
}

impl PreviewRenderer {
    pub(crate) fn open(
        input: &[String],
        options: ConvertOptions<'_>,
        cache_bytes: usize,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let ConvertOptions {
            src_crs,
            nodata,
            resampling,
            tile_format,
            avif_quality,
            avif_speed,
            png_compression,
            ..
        } = options;

        let cli_nodata = parse_nodata(nodata)?;
        let sources_meta = crate::resample::load_source_metadata(input, src_crs)?;
        let nodata = super::resolve_nodata(&sources_meta, cli_nodata)?;
        let source_paths: Vec<_> = sources_meta.iter().map(|meta| meta.path.clone()).collect();
        let (decoders, layouts) = open_sources(&source_paths)?;
        let source_specs: Vec<SourceSpec> = sources_meta
            .into_iter()
            .zip(layouts.iter().cloned())
            .map(|(meta, layout)| SourceSpec::from_meta(meta, layout))
            .collect();

        let mut source_bounds = Vec::with_capacity(source_specs.len());
        for source in &source_specs {
            let corners = source_corners_merc_georef(&source.georef, source.width, source.height)?;
            source_bounds.push(bounds_for_corners(corners));
        }

        Ok(Self {
            source_specs,
            source_bounds,
            layouts,
            io: Mutex::new(PreviewIo {
                decoders,
                cache: ChunkLruCache::new(cache_bytes),
            }),
            nodata,
            resampling,
            tile_format,
            avif_encoder: match tile_format {
                TileFormat::Avif => {
                    Some(crate::resample::make_avif_encoder(avif_speed, avif_quality))
                }
                TileFormat::Png => None,
            },
            png_compression,
        })
    }

    pub(crate) fn tile_format(&self) -> TileFormat {
        self.tile_format
    }

    pub(crate) fn render_tile(
        &self,
        z: u8,
        x: u32,
        y: u32,
    ) -> Result<Option<Vec<u8>>, Box<dyn std::error::Error>> {
        if z > 31 {
            return Err(format!("zoom must be <= 31, got {z}").into());
        }
        let dimension = 1_u64 << z;
        if u64::from(x) >= dimension || u64::from(y) >= dimension {
            return Err(format!("tile coordinate is outside zoom {z}: {x}/{y}").into());
        }

        let (needed_chunks, selected) =
            compute_chunk_requirements((z, x, y), &self.source_specs, &self.source_bounds);
        if selected.is_empty() {
            return Ok(None);
        }

        let chunk_map = self.load_chunks(needed_chunks)?;
        let mut samplers = make_samplers(&self.source_specs);
        let mut rgba = Vec::with_capacity(TILE_SIZE * TILE_SIZE * 4);
        render_tile_chunked(
            &mut samplers,
            &selected,
            self.resampling,
            self.nodata,
            &chunk_map,
            &mut rgba,
        )?;

        if rgba.chunks_exact(4).all(|pixel| pixel[3] == 0) {
            return Ok(None);
        }

        let encoded = match self.tile_format {
            TileFormat::Avif => {
                let encoder = self
                    .avif_encoder
                    .as_ref()
                    .expect("AVIF encoder is initialized for AVIF output");
                crate::resample::encode_avif(encoder, &rgba)?
            }
            TileFormat::Png => {
                let compression = match self.png_compression {
                    PngCompression::Fast => png::Compression::Fast,
                    PngCompression::Balanced => png::Compression::Balanced,
                    PngCompression::High => png::Compression::High,
                };
                crate::resample::encode_png(&rgba, compression)?
            }
        };
        Ok(Some(encoded))
    }

    fn load_chunks(
        &self,
        needed_chunks: HashSet<ChunkKey>,
    ) -> Result<HashMap<ChunkKey, Arc<ChunkData>>, Box<dyn std::error::Error>> {
        let mut io = self
            .io
            .lock()
            .map_err(|_| std::io::Error::other("preview TIFF state lock was poisoned"))?;
        let mut chunk_map = HashMap::with_capacity(needed_chunks.len());
        let mut missing = HashSet::new();

        for key in needed_chunks {
            if let Some(chunk) = io.cache.get(&key) {
                chunk_map.insert(key, chunk);
            } else {
                missing.insert(key);
            }
        }

        if !missing.is_empty() {
            let loaded = super::read_chunks(&missing, &mut io.decoders, &self.layouts)?;
            for (key, chunk) in loaded {
                io.cache.insert(key, Arc::clone(&chunk));
                chunk_map.insert(key, chunk);
            }
        }

        Ok(chunk_map)
    }
}

fn bounds_for_corners(corners: [Pt; 4]) -> (f64, f64, f64, f64) {
    let min_x = corners
        .iter()
        .map(|point| point.x)
        .fold(f64::INFINITY, f64::min);
    let max_x = corners
        .iter()
        .map(|point| point.x)
        .fold(f64::NEG_INFINITY, f64::max);
    let min_y = corners
        .iter()
        .map(|point| point.y)
        .fold(f64::INFINITY, f64::min);
    let max_y = corners
        .iter()
        .map(|point| point.y)
        .fold(f64::NEG_INFINITY, f64::max);
    (min_x, min_y, max_x, max_y)
}
