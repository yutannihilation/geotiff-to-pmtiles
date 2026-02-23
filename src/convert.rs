use std::collections::BTreeSet;
use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;

use pmtiles::{Compression, PmTilesWriter, TileCoord, TileId, TileType};
use proj_lite::Proj;
use tiff::decoder::Decoder;
use tiff::tags::Tag;

use crate::cli::Resampling;
use crate::resample::{
    Georef, NoDataSpec, Pt, decoding_result_to_u8, encode_avif, lerp, load_source_metadata,
    parse_nodeta, source_corners_merc_meta, tile_bounds_webmerc,
    tile_corners_in_source_raster_meta, webmerc_to_tile, zoom_for_tile_size,
};

const TILE_SIZE: usize = 512;
// Global cache budget shared across all input sources/chunks.
// This caps memory growth when many TIFF files are involved.
const DEFAULT_GLOBAL_CHUNK_CACHE_BYTES: usize = 128 * 1024 * 1024;

struct ChunkData {
    width: usize,
    height: usize,
    stride: usize,
    data: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ChunkKey {
    source_idx: usize,
    chunk_idx: u32,
}

struct GlobalChunkCache {
    max_bytes: usize,
    used_bytes: usize,
    order: VecDeque<ChunkKey>,
    map: HashMap<ChunkKey, ChunkData>,
}

impl GlobalChunkCache {
    fn new(max_bytes: usize) -> Self {
        Self {
            max_bytes,
            used_bytes: 0,
            order: VecDeque::new(),
            map: HashMap::new(),
        }
    }

    fn get(&mut self, key: ChunkKey) -> Option<&ChunkData> {
        if self.map.contains_key(&key) {
            self.touch(key);
        }
        self.map.get(&key)
    }

    fn insert(&mut self, key: ChunkKey, value: ChunkData) {
        let value_bytes = value.data.len();
        if self.map.contains_key(&key) {
            self.order.retain(|k| *k != key);
            if let Some(old) = self.map.remove(&key) {
                self.used_bytes = self.used_bytes.saturating_sub(old.data.len());
            }
        }
        // LRU eviction by total bytes, not item count.
        while self.used_bytes + value_bytes > self.max_bytes {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(old) = self.map.remove(&oldest) {
                self.used_bytes = self.used_bytes.saturating_sub(old.data.len());
            }
        }
        self.used_bytes += value_bytes;
        self.map.insert(key, value);
        self.order.push_back(key);
    }

    fn touch(&mut self, key: ChunkKey) {
        self.order.retain(|k| *k != key);
        self.order.push_back(key);
    }
}

enum SourceReader {
    Chunked(ChunkedTiffSampler),
    FullDeferred(Option<crate::resample::Raster>),
}

struct SourceSampler {
    path: PathBuf,
    georef: Georef,
    width: usize,
    height: usize,
    reader: SourceReader,
}

struct ChunkedTiffSampler {
    decoder: Decoder<BufReader<File>>,
    width: usize,
    height: usize,
    samples: usize,
    chunk_w: usize,
    chunk_h: usize,
    chunks_across: usize,
    chunk_count: usize,
}

impl ChunkedTiffSampler {
    fn open(
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
        let chunk_count = match decoder.get_chunk_type() {
            tiff::decoder::ChunkType::Strip => decoder.strip_count()? as usize,
            tiff::decoder::ChunkType::Tile => decoder.tile_count()? as usize,
        };

        Ok(Self {
            decoder,
            width,
            height,
            samples,
            chunk_w,
            chunk_h,
            chunks_across,
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
    fn sample_pixel_opt(
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
                if cache.get(key).is_none() {
                    let chunk = s.read_chunk_data(chunk_idx)?;
                    cache.insert(key, chunk);
                }
                Ok(cache
                    .get(key)
                    .and_then(|chunk| pixel_from_chunk(chunk, lx, ly)))
            }
            SourceReader::FullDeferred(raster_opt) => {
                if raster_opt.is_none() {
                    *raster_opt = Some(crate::resample::load_raster(self.path.as_path())?);
                }
                Ok(raster_opt
                    .as_ref()
                    .and_then(|raster| sample_pixel_raster_opt(raster, xi, yi)))
            }
        }
    }

    fn release_if_inactive(&mut self, active: bool) {
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
    let sources_meta = load_source_metadata(input, src_crs)?;
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
        let mut tile_list = Vec::with_capacity(tiles.len());
        for (x, y) in tiles {
            let coord = TileCoord::new(z, x, y)?;
            let tile_id = TileId::from(coord).value();
            tile_list.push((tile_id, x, y));
        }
        tile_list.sort_by_key(|(tile_id, _, _)| *tile_id);

        let total_tiles = tile_list.len();
        println!("z={z}: rendering {total_tiles} tile(s) [0%]");

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
            let avif = encode_avif(&rgba)?;
            let coord = TileCoord::new(z, x, y)?;
            writer.add_raw_tile(coord, &avif)?;

            for (i, source) in sources.iter_mut().enumerate() {
                source.release_if_inactive(active_sources[i]);
            }

            let done = idx + 1;
            let percent = (done * 100) / total_tiles.max(1);
            let bucket = percent / 10;
            if bucket > reported_bucket {
                reported_bucket = bucket;
                println!("z={z}: {percent}% ({done}/{total_tiles})");
            }
        }
        println!("z={z}: complete [100%]");
    }

    writer.finalize()?;
    Ok(())
}

fn render_tile_chunked(
    sources: &mut [SourceSampler],
    selected: &[(usize, [Pt; 4])],
    resampling: Resampling,
    nodata: Option<NoDataSpec>,
    cache: &mut GlobalChunkCache,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut out = vec![0_u8; TILE_SIZE * TILE_SIZE * 4];
    for j in 0..TILE_SIZE {
        let v = if TILE_SIZE > 1 {
            j as f64 / (TILE_SIZE as f64 - 1.0)
        } else {
            0.0
        };
        for i in 0..TILE_SIZE {
            let u = if TILE_SIZE > 1 {
                i as f64 / (TILE_SIZE as f64 - 1.0)
            } else {
                0.0
            };
            let px = match resampling {
                Resampling::Nearest => {
                    sample_nearest_multi(sources, selected, u, v, nodata, cache)?
                }
                Resampling::Bilinear => {
                    sample_bilinear_multi(sources, selected, u, v, nodata, cache)?
                }
            };
            let base = (j * TILE_SIZE + i) * 4;
            out[base] = px[0];
            out[base + 1] = px[1];
            out[base + 2] = px[2];
            out[base + 3] = px[3];
        }
    }
    Ok(out)
}

fn sample_nearest_multi(
    samplers: &mut [SourceSampler],
    selected: &[(usize, [Pt; 4])],
    u: f64,
    v: f64,
    nodata: Option<NoDataSpec>,
    cache: &mut GlobalChunkCache,
) -> Result<[u8; 4], Box<dyn std::error::Error>> {
    let mut best: Option<([u8; 4], f64)> = None;
    for (idx, corners) in selected {
        let left = lerp(corners[0], corners[3], v);
        let right = lerp(corners[1], corners[2], v);
        let p = lerp(left, right, u);
        if let Some((px, dist2)) =
            sample_nearest_with_dist(samplers, *idx, p.x, p.y, nodata, cache)?
        {
            match best {
                Some((_, d)) if d <= dist2 => {}
                _ => best = Some((px, dist2)),
            }
        }
    }
    Ok(best.map(|(px, _)| px).unwrap_or([0, 0, 0, 0]))
}

fn sample_bilinear_multi(
    samplers: &mut [SourceSampler],
    selected: &[(usize, [Pt; 4])],
    u: f64,
    v: f64,
    nodata: Option<NoDataSpec>,
    cache: &mut GlobalChunkCache,
) -> Result<[u8; 4], Box<dyn std::error::Error>> {
    for (idx, corners) in selected {
        let left = lerp(corners[0], corners[3], v);
        let right = lerp(corners[1], corners[2], v);
        let p = lerp(left, right, u);
        if let Some(px) = sample_bilinear_opt(samplers, *idx, p.x, p.y, nodata, cache)? {
            return Ok(px);
        }
    }
    Ok([0, 0, 0, 0])
}

fn sample_nearest_with_dist(
    samplers: &mut [SourceSampler],
    source_idx: usize,
    x: f64,
    y: f64,
    nodata: Option<NoDataSpec>,
    cache: &mut GlobalChunkCache,
) -> Result<Option<([u8; 4], f64)>, Box<dyn std::error::Error>> {
    let x0 = x.floor() as isize;
    let y0 = y.floor() as isize;
    let mut candidates = [(x0, y0), (x0 + 1, y0), (x0, y0 + 1), (x0 + 1, y0 + 1)];
    candidates.sort_by(|(ax, ay), (bx, by)| {
        let da = (*ax as f64 - x).powi(2) + (*ay as f64 - y).powi(2);
        let db = (*bx as f64 - x).powi(2) + (*by as f64 - y).powi(2);
        da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
    });

    for (xi, yi) in candidates {
        let Some(px) = samplers[source_idx].sample_pixel_opt(source_idx, xi, yi, cache)? else {
            continue;
        };
        if let Some(nd) = nodata {
            if nd.is_nodata(px) {
                continue;
            }
        }
        let dist2 = (xi as f64 - x).powi(2) + (yi as f64 - y).powi(2);
        return Ok(Some((px, dist2)));
    }
    Ok(None)
}

fn sample_bilinear_opt(
    samplers: &mut [SourceSampler],
    source_idx: usize,
    x: f64,
    y: f64,
    nodata: Option<NoDataSpec>,
    cache: &mut GlobalChunkCache,
) -> Result<Option<[u8; 4]>, Box<dyn std::error::Error>> {
    let x0 = x.floor();
    let y0 = y.floor();
    let x1 = x0 + 1.0;
    let y1 = y0 + 1.0;
    let tx = x - x0;
    let ty = y - y0;

    let samples = [
        (
            samplers[source_idx].sample_pixel_opt(source_idx, x0 as isize, y0 as isize, cache)?,
            (1.0 - tx) * (1.0 - ty),
        ),
        (
            samplers[source_idx].sample_pixel_opt(source_idx, x1 as isize, y0 as isize, cache)?,
            tx * (1.0 - ty),
        ),
        (
            samplers[source_idx].sample_pixel_opt(source_idx, x0 as isize, y1 as isize, cache)?,
            (1.0 - tx) * ty,
        ),
        (
            samplers[source_idx].sample_pixel_opt(source_idx, x1 as isize, y1 as isize, cache)?,
            tx * ty,
        ),
    ];

    let mut acc = [0.0_f64; 4];
    let mut wsum = 0.0_f64;
    for (px, w) in samples {
        let Some(px) = px else {
            continue;
        };
        if let Some(nd) = nodata {
            if nd.is_nodata(px) {
                continue;
            }
        }
        wsum += w;
        for c in 0..4 {
            acc[c] += px[c] as f64 * w;
        }
    }

    if wsum <= f64::EPSILON {
        return Ok(None);
    }

    let mut out = [0_u8; 4];
    for c in 0..4 {
        out[c] = (acc[c] / wsum).round().clamp(0.0, 255.0) as u8;
    }
    Ok(Some(out))
}
