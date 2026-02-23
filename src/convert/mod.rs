mod cache;
mod render;
mod source;

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::sync::mpsc;

use pmtiles::{Compression, PmTilesWriter, TileCoord, TileId, TileType};
use proj_lite::Proj;

use crate::cli::Resampling;
use crate::resample::{
    Georef, parse_nodeta, source_corners_merc_meta, tile_bounds_webmerc,
    tile_corners_in_source_raster_meta, webmerc_to_tile, zoom_for_tile_size,
};

use self::cache::GlobalChunkCache;
use self::render::render_tile_chunked;
use self::source::{ChunkedTiffSampler, SourceReader, SourceSampler};

const TILE_SIZE: usize = 512;
// Global cache budget shared across all input sources/chunks.
// This caps memory growth when many TIFF files are involved.
const DEFAULT_GLOBAL_CHUNK_CACHE_BYTES: usize = 128 * 1024 * 1024;

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
    let mut sources = Vec::with_capacity(sources_meta.len());
    for src in sources_meta {
        let reader = match ChunkedTiffSampler::open(src.path.as_path(), src.width, src.height) {
            Ok(sampler) => SourceReader::Chunked(sampler),
            Err(_) => {
                // Fallback path for unsupported layouts (e.g. planar TIFF).
                SourceReader::FullDeferred(None)
            }
        };
        sources.push(SourceSampler {
            path: src.path,
            georef: src.georef,
            width: src.width,
            height: src.height,
            reader,
        });
    }

    let mut corners_merc = Vec::new();
    let mut source_bounds = Vec::with_capacity(sources.len());
    for source in &sources {
        // Build per-source bbox in Web Mercator once so each tile can cheaply cull non-overlapping
        // datasets before attempting any raster sampling.
        let meta = crate::resample::SourceMetadata {
            path: source.path.clone(),
            width: source.width,
            height: source.height,
            georef: Georef {
                source_crs: source.georef.source_crs.clone(),
                forward: source.georef.forward,
                raster_offset: source.georef.raster_offset,
            },
        };
        let corners = source_corners_merc_meta(&meta)?;
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
    println!("Input files: {}", sources.len());
    println!("Output: {}", output.display());
    println!("Zoom range: {min_zoom}..{max_zoom}");
    let cache_bytes = if cache_mb == 0 {
        DEFAULT_GLOBAL_CHUNK_CACHE_BYTES
    } else {
        cache_mb.saturating_mul(1024 * 1024)
    };
    println!("Chunk cache budget: {} MiB", cache_bytes / (1024 * 1024));
    let mut global_cache = GlobalChunkCache::new(cache_bytes);

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
        let max_in_flight = rayon::current_num_threads().max(1).saturating_mul(2);
        let mut in_flight = 0usize;
        let mut next_to_write = 0usize;
        let mut ready = BTreeMap::new();

        let mut reported_bucket = 0usize;
        for (idx, (_tile_id, x, y)) in tile_list.into_iter().enumerate() {
            let bounds = tile_bounds_webmerc(z, x, y);
            let tile_merc_corners = [bounds.ul, bounds.ur, bounds.lr, bounds.ll];
            let tile_min_x = bounds.ul.x.min(bounds.lr.x);
            let tile_max_x = bounds.ul.x.max(bounds.lr.x);
            let tile_min_y = bounds.ul.y.min(bounds.lr.y);
            let tile_max_y = bounds.ul.y.max(bounds.lr.y);

            let mut loaded_sources = Vec::new();
            let mut active_sources = vec![false; sources.len()];
            for (source_idx, (_source, (smin_x, smin_y, smax_x, smax_y))) in
                sources.iter().zip(source_bounds.iter()).enumerate()
            {
                // Skip sources whose mercator bbox does not overlap this output tile.
                let intersects = !(tile_max_x < *smin_x
                    || tile_min_x > *smax_x
                    || tile_max_y < *smin_y
                    || tile_min_y > *smax_y);
                if !intersects {
                    continue;
                }
                let meta = crate::resample::SourceMetadata {
                    path: sources[source_idx].path.clone(),
                    width: sources[source_idx].width,
                    height: sources[source_idx].height,
                    georef: Georef {
                        source_crs: sources[source_idx].georef.source_crs.clone(),
                        forward: sources[source_idx].georef.forward,
                        raster_offset: sources[source_idx].georef.raster_offset,
                    },
                };
                let corners = tile_corners_in_source_raster_meta(&meta, tile_merc_corners)?;
                loaded_sources.push((source_idx, corners));
                active_sources[source_idx] = true;
            }

            let rgba = render_tile_chunked(
                &mut sources,
                &loaded_sources,
                resampling,
                nodata,
                &mut global_cache,
            )?;

            // Release fallback full-raster buffers once the source is no longer touched by
            // subsequent tiles in this zoom loop.
            for (i, source) in sources.iter_mut().enumerate() {
                source.release_if_inactive(active_sources[i]);
            }

            let tx = encoded_tx.clone();
            rayon::spawn(move || {
                let encoded = crate::resample::encode_avif(&rgba).map_err(|e| e.to_string());
                let _ = tx.send((idx, x, y, encoded));
            });
            in_flight += 1;

            while in_flight >= max_in_flight {
                let (done_idx, done_x, done_y, encoded) = encoded_rx
                    .recv()
                    .map_err(|e| format!("encoder worker disconnected: {e}"))?;
                in_flight = in_flight.saturating_sub(1);
                let avif = encoded.map_err(|msg| format!("avif encode failed: {msg}"))?;
                ready.insert(done_idx, (done_x, done_y, avif));
                while let Some((write_x, write_y, bytes)) = ready.remove(&next_to_write) {
                    let coord = TileCoord::new(z, write_x, write_y)?;
                    writer.add_raw_tile(coord, &bytes)?;
                    next_to_write += 1;
                }
            }

            let done = idx + 1;
            let percent = (done * 100) / total_tiles.max(1);
            let bucket = percent / 10;
            if bucket > reported_bucket {
                reported_bucket = bucket;
                println!("z={z}: {percent}% ({done}/{total_tiles})");
            }
        }
        drop(encoded_tx);
        while in_flight > 0 {
            let (done_idx, done_x, done_y, encoded) = encoded_rx
                .recv()
                .map_err(|e| format!("encoder worker disconnected: {e}"))?;
            in_flight = in_flight.saturating_sub(1);
            let avif = encoded.map_err(|msg| format!("avif encode failed: {msg}"))?;
            ready.insert(done_idx, (done_x, done_y, avif));
            while let Some((write_x, write_y, bytes)) = ready.remove(&next_to_write) {
                let coord = TileCoord::new(z, write_x, write_y)?;
                writer.add_raw_tile(coord, &bytes)?;
                next_to_write += 1;
            }
        }
        println!("z={z}: complete [100%]");
    }

    writer.finalize()?;
    Ok(())
}
