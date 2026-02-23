mod cache;
mod render;
mod source;

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::sync::mpsc;

use pmtiles::{Compression, PmTilesWriter, TileCoord, TileId, TileType};
use proj_lite::Proj;
use rayon::prelude::*;

use crate::cli::Resampling;
use crate::resample::{
    GeoTransform, Georef, Pt, SourceMetadata, parse_nodeta, source_corners_merc_meta,
    tile_bounds_webmerc, tile_corners_in_source_raster_meta, webmerc_to_tile, zoom_for_tile_size,
};

use self::cache::GlobalChunkCache;
use self::render::render_tile_chunked;
use self::source::{ChunkedTiffSampler, SourceReader, SourceSampler};

const TILE_SIZE: usize = 512;
// Global cache budget shared across all input sources/chunks.
// This caps memory growth when many TIFF files are involved.
const DEFAULT_GLOBAL_CHUNK_CACHE_BYTES: usize = 128 * 1024 * 1024;

struct SourceSpec {
    path: std::path::PathBuf,
    width: usize,
    height: usize,
    source_crs: String,
    forward: GeoTransform,
    raster_offset: f64,
}

impl SourceSpec {
    fn from_meta(meta: SourceMetadata) -> Self {
        Self {
            path: meta.path,
            width: meta.width,
            height: meta.height,
            source_crs: meta.georef.source_crs,
            forward: meta.georef.forward,
            raster_offset: meta.georef.raster_offset,
        }
    }

    fn as_meta(&self) -> SourceMetadata {
        SourceMetadata {
            path: self.path.clone(),
            width: self.width,
            height: self.height,
            georef: Georef {
                source_crs: self.source_crs.clone(),
                forward: self.forward,
                raster_offset: self.raster_offset,
            },
        }
    }
}

struct WorkerState {
    sources: Vec<SourceSampler>,
    cache: GlobalChunkCache,
}

impl WorkerState {
    fn new(specs: &[SourceSpec], cache_bytes: usize) -> Self {
        let mut sources = Vec::with_capacity(specs.len());
        for spec in specs {
            let reader =
                match ChunkedTiffSampler::open(spec.path.as_path(), spec.width, spec.height) {
                    Ok(sampler) => SourceReader::Chunked(sampler),
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
        z: u8,
        x: u32,
        y: u32,
        source_specs: &[SourceSpec],
        source_bounds: &[(f64, f64, f64, f64)],
        resampling: Resampling,
        nodata: Option<crate::resample::NoDataSpec>,
    ) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
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
            let corners = tile_corners_in_source_raster_meta(&spec.as_meta(), tile_merc_corners)?;
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
        crate::resample::encode_avif(&rgba)
    }
}

pub fn convert(
    input: &[String],
    output: &std::path::Path,
    src_crs: Option<&str>,
    nodeta: Option<&str>,
    min_zoom_opt: Option<u8>,
    max_zoom_opt: Option<u8>,
    resampling: Resampling,
    cache_mb: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let nodata = parse_nodeta(nodeta)?;
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
        let corners = source_corners_merc_meta(&source.as_meta())?;
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
    let cache_bytes = if cache_mb == 0 {
        DEFAULT_GLOBAL_CHUNK_CACHE_BYTES
    } else {
        cache_mb.saturating_mul(1024 * 1024)
    };
    println!("Chunk cache budget: {} MiB", cache_bytes / (1024 * 1024));
    let workers = rayon::current_num_threads().max(1);
    let worker_cache_bytes = (cache_bytes / workers).max(1);

    for z in min_zoom..=max_zoom {
        // Cover the full extent by taking the inclusive tile range from bbox corners.
        let (x_min, y_min) = webmerc_to_tile(min_x_merc, max_y_merc, z);
        let (x_max, y_max) = webmerc_to_tile(max_x_merc, min_y_merc, z);

        let mut tiles = BTreeSet::new();
        for y in y_min..=y_max {
            for x in x_min..=x_max {
                tiles.insert((x, y));
            }
        }
        // Sort by PMTiles tile id for deterministic write order.
        let mut tile_list = Vec::with_capacity(tiles.len());
        for (x, y) in tiles {
            let coord = TileCoord::new(z, x, y)?;
            let tile_id = TileId::from(coord).value();
            tile_list.push((tile_id, x, y));
        }
        tile_list.sort_by_key(|(tile_id, _, _)| *tile_id);

        let total_tiles = tile_list.len();
        println!("z={z}: rendering {total_tiles} tile(s) [0%]");
        let (encoded_tx, encoded_rx) =
            mpsc::channel::<(usize, u32, u32, Result<Vec<u8>, String>)>();
        let mut next_to_write = 0usize;
        let mut ready = BTreeMap::new();
        let mut render_error: Option<String> = None;
        let source_specs_ref = &source_specs;
        let source_bounds_ref = &source_bounds;
        std::thread::scope(|scope| {
            let tx = encoded_tx.clone();
            scope.spawn(move || {
                tile_list
                    .par_iter()
                    .enumerate()
                    .map_init(
                        || WorkerState::new(source_specs_ref, worker_cache_bytes),
                        |worker, (idx, (_tile_id, x, y))| {
                            let encoded = worker
                                .render_and_encode_tile(
                                    z,
                                    *x,
                                    *y,
                                    source_specs_ref,
                                    source_bounds_ref,
                                    resampling,
                                    nodata,
                                )
                                .map_err(|e| e.to_string());
                            let _ = tx.send((idx, *x, *y, encoded));
                        },
                    )
                    .for_each(|_| {});
            });

            drop(encoded_tx);
            let mut reported_bucket = 0usize;
            while next_to_write < total_tiles {
                let (done_idx, done_x, done_y, encoded) = match encoded_rx.recv() {
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
                ready.insert(done_idx, (done_x, done_y, avif));
                while let Some((write_x, write_y, bytes)) = ready.remove(&next_to_write) {
                    let coord = match TileCoord::new(z, write_x, write_y) {
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
                        println!("z={z}: {percent}% ({next_to_write}/{total_tiles})");
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
            return Err(
                format!("z={z}: expected {total_tiles} tile(s), wrote {next_to_write}").into(),
            );
        }
        println!("z={z}: complete [100%]");
    }

    writer.finalize()?;
    Ok(())
}
