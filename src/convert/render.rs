use crate::cli::Resampling;
use crate::resample::{NoDataSpec, Pt, lerp};

use super::TILE_SIZE;
use super::cache::GlobalChunkCache;
use super::source::SourceSampler;

type NearestSample = ([u8; 4], f64);

#[derive(Clone, Copy)]
struct RowSampleCursor {
    source_idx: usize,
    x: f64,
    y: f64,
    dx: f64,
    dy: f64,
}

pub(super) fn render_tile_chunked(
    sources: &mut [SourceSampler],
    selected: &[(usize, [Pt; 4])],
    resampling: Resampling,
    nodata: Option<NoDataSpec>,
    cache: &mut GlobalChunkCache,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    // Render one TILE_SIZE x TILE_SIZE output tile in scanline order.
    let mut out = vec![0_u8; TILE_SIZE * TILE_SIZE * 4];
    let denom = (TILE_SIZE as f64 - 1.0).max(1.0);
    for j in 0..TILE_SIZE {
        let v = if TILE_SIZE > 1 { j as f64 / denom } else { 0.0 };
        let mut cursors = build_row_cursors(selected, v, denom);
        match resampling {
            Resampling::Nearest => {
                for i in 0..TILE_SIZE {
                    let px = sample_nearest_multi(sources, &cursors, nodata, cache)?;
                    let base = (j * TILE_SIZE + i) * 4;
                    out[base] = px[0];
                    out[base + 1] = px[1];
                    out[base + 2] = px[2];
                    out[base + 3] = px[3];
                    advance_cursors(&mut cursors);
                }
            }
            Resampling::Bilinear => {
                for i in 0..TILE_SIZE {
                    let px = sample_bilinear_multi(sources, &cursors, nodata, cache)?;
                    let base = (j * TILE_SIZE + i) * 4;
                    out[base] = px[0];
                    out[base + 1] = px[1];
                    out[base + 2] = px[2];
                    out[base + 3] = px[3];
                    advance_cursors(&mut cursors);
                }
            }
        }
    }
    Ok(out)
}

fn build_row_cursors(selected: &[(usize, [Pt; 4])], v: f64, denom: f64) -> Vec<RowSampleCursor> {
    // Precompute per-row affine stepping so inner loops use only additions.
    let mut cursors = Vec::with_capacity(selected.len());
    for (source_idx, corners) in selected {
        let left = lerp(corners[0], corners[3], v);
        let right = lerp(corners[1], corners[2], v);
        let dx = (right.x - left.x) / denom;
        let dy = (right.y - left.y) / denom;
        cursors.push(RowSampleCursor {
            source_idx: *source_idx,
            x: left.x,
            y: left.y,
            dx,
            dy,
        });
    }
    cursors
}

fn advance_cursors(cursors: &mut [RowSampleCursor]) {
    // Hot-path update used once per output pixel.
    for c in cursors {
        c.x += c.dx;
        c.y += c.dy;
    }
}

fn sample_nearest_multi(
    samplers: &mut [SourceSampler],
    cursors: &[RowSampleCursor],
    nodata: Option<NoDataSpec>,
    cache: &mut GlobalChunkCache,
) -> Result<[u8; 4], Box<dyn std::error::Error>> {
    // Nearest policy across sources: choose globally nearest valid sample in raster pixel space.
    let mut best: Option<NearestSample> = None;
    for c in cursors {
        if let Some((px, dist2)) =
            sample_nearest_with_dist(samplers, c.source_idx, c.x, c.y, nodata, cache)?
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
    cursors: &[RowSampleCursor],
    nodata: Option<NoDataSpec>,
    cache: &mut GlobalChunkCache,
) -> Result<[u8; 4], Box<dyn std::error::Error>> {
    // Bilinear policy across sources: first source in input order that yields a valid sample wins.
    for c in cursors {
        if let Some(px) = sample_bilinear_opt(samplers, c.source_idx, c.x, c.y, nodata, cache)? {
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
) -> Result<Option<NearestSample>, Box<dyn std::error::Error>> {
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
        if let Some(nd) = nodata
            && nd.is_nodata(px)
        {
            continue;
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

    // Weighted 2x2 neighborhood around (x, y); invalid/nodata neighbors are skipped.
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
        if let Some(nd) = nodata
            && nd.is_nodata(px)
        {
            continue;
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
