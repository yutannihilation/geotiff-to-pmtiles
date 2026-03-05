pub(crate) mod cache;
mod render;
mod source;

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::sync::mpsc;

use pmtiles::{Compression, PmTilesWriter, TileCoord, TileId, TileType};
use proj_lite::Proj;
use rayon::prelude::*;
use tiff_compio::ChunkLayout;

use crate::cli::Resampling;
use crate::resample::{
    Georef, Pt, SourceMetadata, TILE_SIZE, parse_nodata, source_corners_merc_georef,
    tile_bounds_webmerc, tile_corners_in_georef_raster, webmerc_to_tile, zoom_for_tile_size,
};

use self::cache::{ChunkData, ChunkKey};
use self::render::render_tile_chunked;
use self::source::{ChunkedTiffSampler, SourceReader, SourceSampler};

/// Default batch size for tile processing. Controls memory: each batch pre-loads
/// all needed TIFF chunks before rendering.
const BATCH_SIZE: usize = 256;

#[allow(dead_code)]
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
    /// Pre-computed chunk layout (None if chunked mode not supported for this source).
    layout: Option<ChunkLayout>,
}

impl SourceSpec {
    fn from_meta(meta: SourceMetadata, layout: Option<ChunkLayout>) -> Self {
        Self {
            path: meta.path,
            width: meta.width,
            height: meta.height,
            georef: meta.georef,
            layout,
        }
    }
}

#[derive(Clone, Copy)]
struct TileJob {
    tile_id: u64,
    write_idx: usize,
    z: u8,
    x: u32,
    y: u32,
    locality_key: u64,
}

fn split_by_1_bits(mut value: u64) -> u64 {
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
    let shift = (max_zoom - z) as u32;
    let nx = (((x as u64) << shift) << 1) | 1;
    let ny = (((y as u64) << shift) << 1) | 1;
    morton_key_2d(nx, ny)
}

/// Compute which TIFF chunks a tile needs from each source.
fn compute_chunk_requirements(
    tile: (u8, u32, u32),
    source_specs: &[SourceSpec],
    source_bounds: &[(f64, f64, f64, f64)],
) -> (HashSet<ChunkKey>, Vec<(usize, [Pt; 4])>) {
    let (z, x, y) = tile;
    let bounds = tile_bounds_webmerc(z, x, y);
    let tile_merc_corners = [bounds.ul, bounds.ur, bounds.lr, bounds.ll];
    let tile_min_x = bounds.ul.x.min(bounds.lr.x);
    let tile_max_x = bounds.ul.x.max(bounds.lr.x);
    let tile_min_y = bounds.ul.y.min(bounds.lr.y);
    let tile_max_y = bounds.ul.y.max(bounds.lr.y);

    let mut needed_chunks = HashSet::new();
    let mut selected_sources = Vec::new();

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

        let corners = match tile_corners_in_georef_raster(&spec.georef, tile_merc_corners) {
            Ok(c) => c,
            Err(_) => continue,
        };

        // Compute raster-space bounding box from the 4 corners
        let mut min_rx = f64::INFINITY;
        let mut max_rx = f64::NEG_INFINITY;
        let mut min_ry = f64::INFINITY;
        let mut max_ry = f64::NEG_INFINITY;
        for c in &corners {
            min_rx = min_rx.min(c.x);
            max_rx = max_rx.max(c.x);
            min_ry = min_ry.min(c.y);
            max_ry = max_ry.max(c.y);
        }

        // Add 1 pixel margin for bilinear sampling
        min_rx = (min_rx - 1.0).max(0.0);
        min_ry = (min_ry - 1.0).max(0.0);
        max_rx = (max_rx + 1.0).min(spec.width as f64);
        max_ry = (max_ry + 1.0).min(spec.height as f64);

        // Map pixel bbox to chunk indices
        if let Some(layout) = &spec.layout {
            let cw = layout.chunk_width as f64;
            let ch = layout.chunk_height as f64;
            let cx_min = (min_rx / cw).floor() as u32;
            let cx_max = (max_rx / cw).floor().min(layout.chunks_across as f64 - 1.0) as u32;
            let cy_min = (min_ry / ch).floor() as u32;
            let cy_max = (max_ry / ch).floor().min(layout.chunks_down as f64 - 1.0) as u32;

            for cy in cy_min..=cy_max {
                for cx in cx_min..=cx_max {
                    let chunk_idx = cy * layout.chunks_across + cx;
                    if chunk_idx < layout.chunk_count {
                        needed_chunks.insert(ChunkKey {
                            source_idx,
                            chunk_idx,
                        });
                    }
                }
            }
        }

        selected_sources.push((source_idx, corners));
    }

    (needed_chunks, selected_sources)
}

/// Read and decompress needed TIFF chunks via compio async I/O.
///
/// Must be called from a compio runtime context (inside `block_on`).
async fn read_chunks_async(
    needed_chunks: &HashSet<ChunkKey>,
    readers: &[tiff_compio::TiffReader<compio::fs::File>],
    layouts: &[Option<ChunkLayout>],
) -> Result<HashMap<ChunkKey, ChunkData>, Box<dyn std::error::Error>> {
    if needed_chunks.is_empty() {
        return Ok(HashMap::new());
    }

    let mut chunk_map = HashMap::with_capacity(needed_chunks.len());
    for key in needed_chunks {
        let layout = layouts[key.source_idx]
            .as_ref()
            .expect("chunk requested for source without layout");
        let raw = readers[key.source_idx]
            .read_chunk(layout, key.chunk_idx)
            .await?;
        let (cw, ch) = layout.chunk_data_dimensions(key.chunk_idx);

        // Normalize to u8
        let bits_per_sample = layout.bits_per_sample[0];
        let sample_format = layout.sample_format;
        let data = tiff_compio::normalize::normalize_to_u8(raw, bits_per_sample, sample_format);

        let cw = cw as usize;
        let ch = ch as usize;
        let samples = layout.samples_per_pixel as usize;
        let stride = if cw == 0 || ch == 0 {
            samples.max(1)
        } else {
            (data.len() / (cw * ch)).max(1)
        };

        chunk_map.insert(
            *key,
            ChunkData {
                width: cw,
                height: ch,
                stride,
                data,
            },
        );
    }

    Ok(chunk_map)
}

/// Build SourceSampler instances (layout metadata only, no file handles).
fn make_samplers(source_specs: &[SourceSpec]) -> Vec<SourceSampler> {
    let mut sources = Vec::with_capacity(source_specs.len());
    for spec in source_specs {
        let reader = if let Some(layout) = &spec.layout {
            match ChunkedTiffSampler::from_layout(layout) {
                Ok(sampler) => SourceReader::Chunked(Box::new(sampler)),
                Err(_) => SourceReader::FullDeferred(None),
            }
        } else {
            SourceReader::FullDeferred(None)
        };
        sources.push(SourceSampler {
            path: spec.path.clone(),
            reader,
        });
    }
    sources
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
        cache_mb: _,
        avif_quality,
        avif_speed,
    } = options;
    let nodata = parse_nodata(nodata)?;

    println!("Input args: {}; loading metadata...", input.join(" "));
    let sources_meta = crate::resample::load_source_metadata(input, src_crs)?;
    println!("Loaded metadata for {} source file(s)", sources_meta.len());

    // Open all source TIFFs with compio and compute layouts once at startup.
    // compio::fs::File is !Send, so readers must stay on the main thread.
    let source_paths: Vec<_> = sources_meta.iter().map(|m| m.path.clone()).collect();
    let rt = compio::runtime::Runtime::new()?;
    let (readers, layouts) = open_sources(&rt, &source_paths)?;

    let source_specs: Vec<SourceSpec> = sources_meta
        .into_iter()
        .zip(layouts.iter().cloned())
        .map(|(meta, layout)| SourceSpec::from_meta(meta, layout))
        .collect();

    let mut corners_merc = Vec::new();
    let mut source_bounds = Vec::with_capacity(source_specs.len());
    for source in &source_specs {
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

    let zoom_span = (max_zoom - min_zoom + 1) as usize;
    let mut skipped_empty_by_zoom = vec![0usize; zoom_span];
    let mut transparent_by_zoom = vec![0usize; zoom_span];
    let mut tiles_by_zoom = vec![0usize; zoom_span];
    let mut jobs = Vec::<TileJob>::new();

    for z in min_zoom..=max_zoom {
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
    for (write_idx, job) in write_jobs.iter_mut().enumerate() {
        job.write_idx = write_idx;
    }

    // Render order: spatial proximity across zoom levels for better chunk cache reuse.
    let mut render_jobs = write_jobs.clone();
    render_jobs.sort_by_key(|job| (job.locality_key, job.z));

    // --- Batch-based pipeline ---
    //
    // compio::fs::File is !Send (uses Rc internally), so all I/O must happen on the
    // main thread. The pipeline processes tiles in batches:
    //
    //   Phase 1 (main thread, CPU): Compute which TIFF chunks each tile needs
    //   Phase 2 (main thread, compio): Read + decompress + normalize all needed chunks
    //   Phase 3 (rayon thread pool): Render tiles from pre-loaded chunks + AVIF encode
    //   Phase 4 (main thread): Write encoded tiles to PMTiles

    let (encoded_tx, encoded_rx) = mpsc::channel::<(usize, Result<Option<Vec<u8>>, String>)>();
    let mut next_to_write = 0usize;
    let mut ready_by_write_idx = vec![None::<Option<Vec<u8>>>; total_tiles];
    let mut render_error: Option<String> = None;
    let mut reported_bucket = 0usize;

    for batch in render_jobs.chunks(BATCH_SIZE) {
        // Phase 1: Compute chunk requirements for all tiles in this batch
        let mut batch_chunks = HashSet::new();
        #[allow(clippy::type_complexity)]
        let batch_selections: Vec<(TileJob, Vec<(usize, [Pt; 4])>)> = batch
            .iter()
            .map(|job| {
                let (chunks, selected) = compute_chunk_requirements(
                    (job.z, job.x, job.y),
                    &source_specs,
                    &source_bounds,
                );
                batch_chunks.extend(chunks);
                (*job, selected)
            })
            .collect();

        // Phase 2: Read all needed chunks via compio (main thread)
        let chunk_map = rt.block_on(read_chunks_async(&batch_chunks, &readers, &layouts))?;

        // Phase 3: Render tiles in parallel with rayon
        let tx = encoded_tx.clone();
        batch_selections
            .par_iter()
            .map_init(
                || make_samplers(&source_specs),
                |sources, (job, selected)| {
                    let active_sources: Vec<bool> = (0..sources.len())
                        .map(|i| selected.iter().any(|(si, _)| *si == i))
                        .collect();

                    let result =
                        render_tile_chunked(sources, selected, resampling, nodata, &chunk_map)
                            .and_then(|rgba| {
                                for (i, source) in sources.iter_mut().enumerate() {
                                    source.release_if_inactive(active_sources[i]);
                                }
                                if rgba.chunks_exact(4).all(|px| px[3] == 0) {
                                    return Ok(None);
                                }
                                Ok(Some(crate::resample::encode_avif(
                                    &rgba,
                                    avif_speed,
                                    avif_quality,
                                )?))
                            })
                            .map_err(|e| e.to_string());

                    let _ = tx.send((job.write_idx, result));
                },
            )
            .for_each(|_| {});

        // Phase 4: Drain results from this batch and write in order
        while let Ok((done_write_idx, encoded)) = encoded_rx.try_recv() {
            let avif = match encoded {
                Ok(avif) => avif,
                Err(msg) => {
                    render_error = Some(format!("tile render/encode failed: {msg}"));
                    break;
                }
            };
            ready_by_write_idx[done_write_idx] = Some(avif);
        }

        // Flush any tiles that are ready to write in order
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
            let coord = TileCoord::new(write_z, write_x, write_y)?;
            writer.add_raw_tile(coord, &bytes)?;
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
    drop(encoded_tx);

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

/// Open all source TIFFs as compio TiffReaders and compute their chunk layouts.
type SourceReaders = (
    Vec<tiff_compio::TiffReader<compio::fs::File>>,
    Vec<Option<ChunkLayout>>,
);

fn open_sources(
    rt: &compio::runtime::Runtime,
    paths: &[std::path::PathBuf],
) -> Result<SourceReaders, Box<dyn std::error::Error>> {
    let mut readers = Vec::with_capacity(paths.len());
    let mut layouts = Vec::with_capacity(paths.len());

    for path in paths {
        let reader = rt.block_on(async {
            let file = compio::fs::File::open(path).await?;
            tiff_compio::TiffReader::new(file).await
        })?;

        let layout = match reader.chunk_layout() {
            Ok(layout) => {
                // Check planar configuration — only chunky is supported
                let planar = reader
                    .find_tag(tiff_compio::tag::PLANAR_CONFIGURATION)
                    .map(|v: tiff_compio::TagValue| v.into_u16())
                    .transpose()?
                    .unwrap_or(1);
                if planar == 2 {
                    None // Fall back to full raster for planar TIFFs
                } else {
                    Some(layout)
                }
            }
            Err(_) => None,
        };

        readers.push(reader);
        layouts.push(layout);
    }

    Ok((readers, layouts))
}
