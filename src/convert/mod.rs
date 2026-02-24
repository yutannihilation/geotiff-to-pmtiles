mod cache;
mod render;
mod source;

use std::fs::File;
use std::sync::mpsc;

use pmtiles::{Compression, PmTilesWriter, TileCoord, TileId, TileType};
use proj_lite::Proj;
use rayon::prelude::*;

use crate::cli::Resampling;
use crate::resample::{
    Georef, Pt, SourceMetadata, TILE_SIZE, parse_nodata, source_corners_merc_georef,
    tile_bounds_webmerc, tile_corners_in_georef_raster, webmerc_to_tile, zoom_for_tile_size,
};

use self::cache::GlobalChunkCache;
use self::render::render_tile_chunked;
use self::source::{ChunkedTiffSampler, SourceReader, SourceSampler};

// Global cache budget shared across all input sources/chunks.
// This caps memory growth when many TIFF files are involved.
const DEFAULT_GLOBAL_CHUNK_CACHE_BYTES: usize = 128 * 1024 * 1024;

pub struct ConvertOptions<'a> {
    pub src_crs: Option<&'a str>,
    pub nodata: Option<&'a str>,
    pub min_zoom: Option<u8>,
    pub max_zoom: Option<u8>,
    pub resampling: Resampling,
    pub cache_mb: usize,
    pub avif_quality: u8,
    pub avif_speed: u8,
}

struct SourceSpec {
    path: std::path::PathBuf,
    width: usize,
    height: usize,
    georef: Georef,
}

impl SourceSpec {
    fn from_meta(meta: SourceMetadata) -> Self {
        Self {
            path: meta.path,
            width: meta.width,
            height: meta.height,
            georef: meta.georef,
        }
    }
}

struct WorkerState {
    sources: Vec<SourceSampler>,
    cache: GlobalChunkCache,
}

#[derive(Clone, Copy)]
struct TileJob {
    tile_id: u64,
    // Position in global tile_id-sorted write order.
    write_idx: usize,
    z: u8,
    x: u32,
    y: u32,
    locality_key: u64,
}

fn split_by_1_bits(mut value: u64) -> u64 {
    // Interleave lower 32 bits: abcdef... -> a0b0c0d0...
    value &= 0x0000_0000_FFFF_FFFF;
    value = (value | (value << 16)) & 0x0000_FFFF_0000_FFFF;
    value = (value | (value << 8)) & 0x00FF_00FF_00FF_00FF;
    value = (value | (value << 4)) & 0x0F0F_0F0F_0F0F_0F0F;
    value = (value | (value << 2)) & 0x3333_3333_3333_3333;
    value = (value | (value << 1)) & 0x5555_5555_5555_5555;
    value
}

fn morton_key_2d(x: u64, y: u64) -> u64 {
    split_by_1_bits(x) | (split_by_1_bits(y) << 1)
}

fn tile_locality_key(z: u8, x: u32, y: u32, max_zoom: u8) -> u64 {
    // Normalize every tile center onto a max_zoom grid to keep nearby areas
    // close in render order even when z differs.
    let shift = (max_zoom - z) as u32;
    let nx = (((x as u64) << shift) << 1) | 1;
    let ny = (((y as u64) << shift) << 1) | 1;
    morton_key_2d(nx, ny)
}

impl WorkerState {
    fn new(specs: &[SourceSpec], cache_bytes: usize) -> Self {
        // Each worker owns its own decoders and cache to avoid lock contention
        // during parallel render/encode.
        let mut sources = Vec::with_capacity(specs.len());
        for spec in specs {
            let reader =
                match ChunkedTiffSampler::open(spec.path.as_path(), spec.width, spec.height) {
                    Ok(sampler) => SourceReader::Chunked(Box::new(sampler)),
                    Err(_) => SourceReader::FullDeferred(None),
                };
            sources.push(SourceSampler {
                path: spec.path.clone(),
                reader,
            });
        }
        Self {
            sources,
            cache: GlobalChunkCache::new(cache_bytes),
        }
    }

    fn render_and_encode_tile(
        &mut self,
        tile: (u8, u32, u32),
        source_specs: &[SourceSpec],
        source_bounds: &[(f64, f64, f64, f64)],
        resampling: Resampling,
        nodata: Option<crate::resample::NoDataSpec>,
        avif_quality: u8,
        avif_speed: u8,
    ) -> Result<Option<Vec<u8>>, Box<dyn std::error::Error>> {
        let (z, x, y) = tile;
        let bounds = tile_bounds_webmerc(z, x, y);
        let tile_merc_corners = [bounds.ul, bounds.ur, bounds.lr, bounds.ll];
        let tile_min_x = bounds.ul.x.min(bounds.lr.x);
        let tile_max_x = bounds.ul.x.max(bounds.lr.x);
        let tile_min_y = bounds.ul.y.min(bounds.lr.y);
        let tile_max_y = bounds.ul.y.max(bounds.lr.y);

        let mut loaded_sources: Vec<(usize, [Pt; 4])> = Vec::new();
        let mut active_sources = vec![false; self.sources.len()];
        for (source_idx, (spec, (smin_x, smin_y, smax_x, smax_y))) in
            source_specs.iter().zip(source_bounds.iter()).enumerate()
        {
            let intersects = !(tile_max_x < *smin_x
                || tile_min_x > *smax_x
                || tile_max_y < *smin_y
                || tile_min_y > *smax_y);
            if !intersects {
                continue;
            }
            let corners = tile_corners_in_georef_raster(&spec.georef, tile_merc_corners)?;
            loaded_sources.push((source_idx, corners));
            active_sources[source_idx] = true;
        }

        let rgba = render_tile_chunked(
            &mut self.sources,
            &loaded_sources,
            resampling,
            nodata,
            &mut self.cache,
        )?;
        for (i, source) in self.sources.iter_mut().enumerate() {
            source.release_if_inactive(active_sources[i]);
        }
        // If every output pixel is transparent, skip emitting this tile entirely.
        // This avoids writing visual "empty" tiles where nodata/out-of-bounds dominates.
        if rgba.chunks_exact(4).all(|px| px[3] == 0) {
            return Ok(None);
        }

        Ok(Some(crate::resample::encode_avif(
            &rgba,
            avif_speed,
            avif_quality,
        )?))
    }
}

pub fn convert(
    input: &[String],
    output: &std::path::Path,
    options: ConvertOptions<'_>,
) -> Result<(), Box<dyn std::error::Error>> {
    let ConvertOptions {
        src_crs,
        nodata,
        min_zoom: min_zoom_opt,
        max_zoom: max_zoom_opt,
        resampling,
        cache_mb,
        avif_quality,
        avif_speed,
    } = options;
    let nodata = parse_nodata(nodata)?;
    // Memory-first strategy:
    // 1) load only metadata up front
    // 2) decode TIFF chunks lazily during sampling
    // 3) keep a global byte-bounded LRU cache of decoded chunks
    println!("Input args: {}; loading metadata...", input.join(" "));
    let sources_meta = crate::resample::load_source_metadata(input, src_crs)?;
    println!("Loaded metadata for {} source file(s)", sources_meta.len());
    let source_specs: Vec<SourceSpec> = sources_meta
        .into_iter()
        .map(SourceSpec::from_meta)
        .collect();

    let mut corners_merc = Vec::new();
    let mut source_bounds = Vec::with_capacity(source_specs.len());
    for source in &source_specs {
        // Build per-source bbox in Web Mercator once so each tile can cheaply cull non-overlapping
        // datasets before attempting any raster sampling.
        let corners = source_corners_merc_georef(&source.georef, source.width, source.height)?;
        let min_x = corners.iter().map(|p| p.x).fold(f64::INFINITY, f64::min);
        let max_x = corners
            .iter()
            .map(|p| p.x)
            .fold(f64::NEG_INFINITY, f64::max);
        let min_y = corners.iter().map(|p| p.y).fold(f64::INFINITY, f64::min);
        let max_y = corners
            .iter()
            .map(|p| p.y)
            .fold(f64::NEG_INFINITY, f64::max);
        source_bounds.push((min_x, min_y, max_x, max_y));
        corners_merc.extend_from_slice(&corners);
    }

    let min_x_merc = corners_merc
        .iter()
        .map(|p| p.x)
        .fold(f64::INFINITY, f64::min);
    let max_x_merc = corners_merc
        .iter()
        .map(|p| p.x)
        .fold(f64::NEG_INFINITY, f64::max);
    let min_y_merc = corners_merc
        .iter()
        .map(|p| p.y)
        .fold(f64::INFINITY, f64::min);
    let max_y_merc = corners_merc
        .iter()
        .map(|p| p.y)
        .fold(f64::NEG_INFINITY, f64::max);

    // Default zoom heuristic: choose the coarsest zoom whose tile edge can still cover
    // the largest side of the union extent. Higher zooms are added as a small pyramid.
    let auto_min_zoom = zoom_for_tile_size((max_x_merc - min_x_merc).max(max_y_merc - min_y_merc));
    let min_zoom = min_zoom_opt.unwrap_or(auto_min_zoom);
    if min_zoom > 31 {
        return Err(format!("min_zoom must be <= 31, got {min_zoom}").into());
    }
    let max_zoom = max_zoom_opt.unwrap_or(min_zoom.saturating_add(3).min(31));
    if max_zoom > 31 {
        return Err(format!("max_zoom must be <= 31, got {max_zoom}").into());
    }
    if max_zoom < min_zoom {
        return Err(format!("max_zoom ({max_zoom}) must be >= min_zoom ({min_zoom})").into());
    }

    let to_wgs84 = Proj::new_known_crs("EPSG:3857", "EPSG:4326")?;
    let mut min_lon = f64::INFINITY;
    let mut min_lat = f64::INFINITY;
    let mut max_lon = f64::NEG_INFINITY;
    let mut max_lat = f64::NEG_INFINITY;
    for p in &corners_merc {
        let (lon, lat) = to_wgs84.transform2((p.x, p.y))?;
        min_lon = min_lon.min(lon);
        min_lat = min_lat.min(lat);
        max_lon = max_lon.max(lon);
        max_lat = max_lat.max(lat);
    }

    let center_lon = (min_lon + max_lon) / 2.0;
    let center_lat = (min_lat + max_lat) / 2.0;

    let file = File::create(output)?;
    let mut writer = PmTilesWriter::new(TileType::Avif)
        .tile_compression(Compression::None)
        .min_zoom(min_zoom)
        .max_zoom(max_zoom)
        .bounds(min_lon, min_lat, max_lon, max_lat)
        .center_zoom(min_zoom)
        .center(center_lon, center_lat)
        .create(file)?;

    println!("Input args: {}", input.join(" "));
    println!("Input files: {}", source_specs.len());
    println!("Output: {}", output.display());
    println!("Zoom range: {min_zoom}..{max_zoom}");
    println!("AVIF: quality={avif_quality}, speed={avif_speed}");
    let cache_bytes = if cache_mb == 0 {
        DEFAULT_GLOBAL_CHUNK_CACHE_BYTES
    } else {
        cache_mb.saturating_mul(1024 * 1024)
    };
    println!("Chunk cache budget: {} MiB", cache_bytes / (1024 * 1024));
    let workers = rayon::current_num_threads().max(1);
    // Split budget across workers so total memory stays close to `cache_mb`.
    let worker_cache_bytes = (cache_bytes / workers).max(1);

    let zoom_span = (max_zoom - min_zoom + 1) as usize;
    let mut skipped_empty_by_zoom = vec![0usize; zoom_span];
    let mut transparent_by_zoom = vec![0usize; zoom_span];
    let mut tiles_by_zoom = vec![0usize; zoom_span];
    let mut jobs = Vec::<TileJob>::new();

    for z in min_zoom..=max_zoom {
        // Cover the full extent by taking the inclusive tile range from bbox corners.
        let (x_min, y_min) = webmerc_to_tile(min_x_merc, max_y_merc, z);
        let (x_max, y_max) = webmerc_to_tile(max_x_merc, min_y_merc, z);

        let tile_capacity = (x_max - x_min + 1) as usize * (y_max - y_min + 1) as usize;
        jobs.reserve(tile_capacity);
        for y in y_min..=y_max {
            for x in x_min..=x_max {
                let bounds = tile_bounds_webmerc(z, x, y);
                let tile_min_x = bounds.ul.x.min(bounds.lr.x);
                let tile_max_x = bounds.ul.x.max(bounds.lr.x);
                let tile_min_y = bounds.ul.y.min(bounds.lr.y);
                let tile_max_y = bounds.ul.y.max(bounds.lr.y);
                let intersects_any =
                    source_bounds
                        .iter()
                        .any(|(smin_x, smin_y, smax_x, smax_y)| {
                            !(tile_max_x < *smin_x
                                || tile_min_x > *smax_x
                                || tile_max_y < *smin_y
                                || tile_min_y > *smax_y)
                        });
                let zi = (z - min_zoom) as usize;
                if !intersects_any {
                    skipped_empty_by_zoom[zi] += 1;
                    continue;
                }
                let coord = TileCoord::new(z, x, y)?;
                let tile_id = TileId::from(coord).value();
                jobs.push(TileJob {
                    tile_id,
                    write_idx: 0,
                    z,
                    x,
                    y,
                    locality_key: tile_locality_key(z, x, y, max_zoom),
                });
                tiles_by_zoom[zi] += 1;
            }
        }
    }

    for z in min_zoom..=max_zoom {
        let zi = (z - min_zoom) as usize;
        let total_tiles = tiles_by_zoom[zi];
        let skipped_empty = skipped_empty_by_zoom[zi];
        if skipped_empty > 0 {
            println!("z={z}: skipped {skipped_empty} empty tile(s)");
        }
        println!("z={z}: rendering {total_tiles} tile(s)");
    }

    let total_tiles = jobs.len();
    println!("rendering {} tile(s) [0%]", total_tiles);

    // Write order: strict PMTiles tile_id ascending for clustered output.
    let mut write_jobs = jobs;
    write_jobs.sort_by_key(|job| job.tile_id);
    // Persist the global output sequence number so render workers can return results
    // without a tile_id->index lookup in the writer thread.
    for (write_idx, job) in write_jobs.iter_mut().enumerate() {
        job.write_idx = write_idx;
    }

    // Render order: spatial proximity across zoom levels for better chunk cache reuse.
    let mut render_jobs = write_jobs.clone();
    render_jobs.sort_by_key(|job| (job.locality_key, job.z));

    let (encoded_tx, encoded_rx) = mpsc::channel::<(usize, Result<Option<Vec<u8>>, String>)>();
    let mut next_to_write = 0usize;
    // O(1) rendezvous buffer keyed by write order index.
    let mut ready_by_write_idx = vec![None::<Option<Vec<u8>>>; total_tiles];
    let mut render_error: Option<String> = None;
    let source_specs_ref = &source_specs;
    let source_bounds_ref = &source_bounds;

    std::thread::scope(|scope| {
        let tx = encoded_tx.clone();
        scope.spawn(move || {
            // `map_init` gives each rayon worker its own mutable worker state.
            render_jobs
                .par_iter()
                .map_init(
                    || WorkerState::new(source_specs_ref, worker_cache_bytes),
                    |worker, job| {
                        let encoded = worker
                            .render_and_encode_tile(
                                (job.z, job.x, job.y),
                                source_specs_ref,
                                source_bounds_ref,
                                resampling,
                                nodata,
                                avif_quality,
                                avif_speed,
                            )
                            .map_err(|e| e.to_string());
                        let _ = tx.send((job.write_idx, encoded));
                    },
                )
                .for_each(|_| {});
        });

        drop(encoded_tx);
        let mut reported_bucket = 0usize;
        while next_to_write < total_tiles {
            let (done_write_idx, encoded) = match encoded_rx.recv() {
                Ok(msg) => msg,
                Err(e) => {
                    render_error = Some(format!("render worker disconnected: {e}"));
                    break;
                }
            };
            let avif = match encoded {
                Ok(avif) => avif,
                Err(msg) => {
                    render_error = Some(format!("tile render/encode failed: {msg}"));
                    break;
                }
            };
            // Store completed result in its final output slot.
            ready_by_write_idx[done_write_idx] = Some(avif);
            while next_to_write < total_tiles {
                let Some(bytes_opt) = ready_by_write_idx[next_to_write].take() else {
                    break;
                };
                let write_job = write_jobs[next_to_write];
                let write_z = write_job.z;
                let write_x = write_job.x;
                let write_y = write_job.y;
                let zi = (write_z - min_zoom) as usize;
                let Some(bytes) = bytes_opt else {
                    transparent_by_zoom[zi] += 1;
                    next_to_write += 1;
                    let percent = (next_to_write * 100) / total_tiles.max(1);
                    let bucket = percent / 10;
                    if bucket > reported_bucket {
                        reported_bucket = bucket;
                        println!("{percent}% ({next_to_write}/{total_tiles})");
                    }
                    continue;
                };
                let coord = match TileCoord::new(write_z, write_x, write_y) {
                    Ok(coord) => coord,
                    Err(e) => {
                        render_error = Some(format!("invalid tile coordinate: {e}"));
                        break;
                    }
                };
                if let Err(e) = writer.add_raw_tile(coord, &bytes) {
                    render_error = Some(format!("failed to write tile: {e}"));
                    break;
                }
                next_to_write += 1;
                let percent = (next_to_write * 100) / total_tiles.max(1);
                let bucket = percent / 10;
                if bucket > reported_bucket {
                    reported_bucket = bucket;
                    println!("{percent}% ({next_to_write}/{total_tiles})");
                }
            }
            if render_error.is_some() {
                break;
            }
        }
    });

    if let Some(err) = render_error {
        return Err(err.into());
    }
    if next_to_write != total_tiles {
        return Err(format!("expected {total_tiles} tile(s), wrote {next_to_write}").into());
    }

    for z in min_zoom..=max_zoom {
        let zi = (z - min_zoom) as usize;
        let skipped_transparent = transparent_by_zoom[zi];
        if skipped_transparent > 0 {
            println!("z={z}: skipped {skipped_transparent} fully transparent tile(s)");
        }
        println!("z={z}: complete [100%]");
    }

    writer.finalize()?;
    Ok(())
}
