use std::collections::HashMap;

use crate::cli::Resampling;
use crate::resample::{NoDataSpec, TileMapping};

use super::TILE_SIZE;
use super::cache::{ChunkData, ChunkKey};
use super::source::SourceSampler;

/// Per-source sampling position for the current output row, advanced by one
/// output pixel at a time. The mapping is piecewise bilinear, so the step
/// `(dx, dy)` is constant only inside one grid cell; `next_switch` is the pixel
/// index at which the cursor must be re-seeded from the next cell.
#[derive(Clone, Copy)]
struct RowSampleCursor {
    source_idx: usize,
    x: f64,
    y: f64,
    dx: f64,
    dy: f64,
    next_switch: usize,
}

pub(super) fn render_tile_chunked(
    sources: &mut [SourceSampler],
    selected: &[(usize, TileMapping)],
    resampling: Resampling,
    nodata: Option<NoDataSpec>,
    chunk_map: &HashMap<ChunkKey, ChunkData>,
    out: &mut Vec<u8>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Render one TILE_SIZE x TILE_SIZE output tile in scanline order.
    out.clear();
    out.resize(TILE_SIZE * TILE_SIZE * 4, 0);
    let denom = (TILE_SIZE as f64 - 1.0).max(1.0);
    let mut cursors = Vec::with_capacity(selected.len());
    for j in 0..TILE_SIZE {
        let v = if TILE_SIZE > 1 { j as f64 / denom } else { 0.0 };
        refill_row_cursors(&mut cursors, selected, v, denom);
        let row = &mut out[j * TILE_SIZE * 4..(j + 1) * TILE_SIZE * 4];
        match resampling {
            Resampling::Nearest => {
                render_row(&mut cursors, selected, v, denom, row, |cursors| {
                    sample_nearest_multi(sources, cursors, nodata, chunk_map)
                })?;
            }
            Resampling::Bilinear => {
                render_row(&mut cursors, selected, v, denom, row, |cursors| {
                    sample_bilinear_multi(sources, cursors, nodata, chunk_map)
                })?;
            }
        }
    }
    Ok(())
}

/// Render one output row. The row is processed in segments between cell
/// switches of any source's cursor, so the inner loop is a plain additive step.
fn render_row<F>(
    cursors: &mut [RowSampleCursor],
    selected: &[(usize, TileMapping)],
    v: f64,
    denom: f64,
    row: &mut [u8],
    mut sample: F,
) -> Result<(), Box<dyn std::error::Error>>
where
    F: FnMut(&[RowSampleCursor]) -> Result<[u8; 4], Box<dyn std::error::Error>>,
{
    let mut i = 0;
    while i < TILE_SIZE {
        let seg_end = cursors
            .iter()
            .map(|c| c.next_switch)
            .min()
            .unwrap_or(TILE_SIZE)
            .min(TILE_SIZE);
        for i in i..seg_end {
            let px = sample(cursors)?;
            row[i * 4..i * 4 + 4].copy_from_slice(&px);
            advance_cursors(cursors);
        }
        i = seg_end;
        if i < TILE_SIZE {
            for (c, (_, mapping)) in cursors.iter_mut().zip(selected) {
                if i >= c.next_switch {
                    *c = seed_cursor(c.source_idx, mapping, v, i, denom);
                }
            }
        }
    }
    Ok(())
}

fn refill_row_cursors(
    cursors: &mut Vec<RowSampleCursor>,
    selected: &[(usize, TileMapping)],
    v: f64,
    denom: f64,
) {
    // Seed one cursor per source at the first pixel of the row.
    cursors.clear();
    for (source_idx, mapping) in selected {
        cursors.push(seed_cursor(*source_idx, mapping, v, 0, denom));
    }
}

/// Cursor positioned exactly at output pixel `i` of the row at UV `v`, with the
/// per-pixel step for the grid cell containing that pixel.
fn seed_cursor(
    source_idx: usize,
    mapping: &TileMapping,
    v: f64,
    i: usize,
    denom: f64,
) -> RowSampleCursor {
    let n = mapping.cells() as f64;
    let u = i as f64 / denom;
    let cu = mapping.cell_index(u);
    let (left, right) = mapping.row_segment(v, cu);
    let lu = u * n - cu as f64;
    // First pixel index whose u lies in the next cell; the last cell runs to the row end.
    let next_switch = if cu + 1 == mapping.cells() {
        TILE_SIZE
    } else {
        (((cu + 1) as f64 * denom / n).ceil() as usize).max(i + 1)
    };
    RowSampleCursor {
        source_idx,
        x: left.x + (right.x - left.x) * lu,
        y: left.y + (right.y - left.y) * lu,
        // One output pixel advances u by 1/denom, i.e. n/denom of the cell width.
        dx: (right.x - left.x) * n / denom,
        dy: (right.y - left.y) * n / denom,
        next_switch,
    }
}

fn advance_cursors(cursors: &mut [RowSampleCursor]) {
    // Hot-path update used once per output pixel inside a grid cell.
    for c in cursors {
        c.x += c.dx;
        c.y += c.dy;
    }
}

fn sample_nearest_multi(
    samplers: &mut [SourceSampler],
    cursors: &[RowSampleCursor],
    nodata: Option<NoDataSpec>,
    chunk_map: &HashMap<ChunkKey, ChunkData>,
) -> Result<[u8; 4], Box<dyn std::error::Error>> {
    // First source in input order that yields a valid (non-nodata) sample wins.
    for c in cursors {
        if let Some(px) = sample_nearest_opt(samplers, c.source_idx, c.x, c.y, nodata, chunk_map)? {
            return Ok(px);
        }
    }
    Ok([0, 0, 0, 0])
}

fn sample_bilinear_multi(
    samplers: &mut [SourceSampler],
    cursors: &[RowSampleCursor],
    nodata: Option<NoDataSpec>,
    chunk_map: &HashMap<ChunkKey, ChunkData>,
) -> Result<[u8; 4], Box<dyn std::error::Error>> {
    // Bilinear policy across sources: first source in input order that yields a valid sample wins.
    for c in cursors {
        if let Some(px) = sample_bilinear_opt(samplers, c.source_idx, c.x, c.y, nodata, chunk_map)?
        {
            return Ok(px);
        }
    }
    Ok([0, 0, 0, 0])
}

fn sample_nearest_opt(
    samplers: &mut [SourceSampler],
    source_idx: usize,
    x: f64,
    y: f64,
    nodata: Option<NoDataSpec>,
    chunk_map: &HashMap<ChunkKey, ChunkData>,
) -> Result<Option<[u8; 4]>, Box<dyn std::error::Error>> {
    let xi = x.round() as isize;
    let yi = y.round() as isize;
    let Some(px) = samplers[source_idx].sample_pixel_opt(source_idx, xi, yi, chunk_map)? else {
        return Ok(None);
    };
    if let Some(nd) = nodata
        && nd.is_nodata(px)
    {
        return Ok(None);
    }
    Ok(Some(px))
}

fn sample_bilinear_opt(
    samplers: &mut [SourceSampler],
    source_idx: usize,
    x: f64,
    y: f64,
    nodata: Option<NoDataSpec>,
    chunk_map: &HashMap<ChunkKey, ChunkData>,
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
            samplers[source_idx].sample_pixel_opt(
                source_idx,
                x0 as isize,
                y0 as isize,
                chunk_map,
            )?,
            (1.0 - tx) * (1.0 - ty),
        ),
        (
            samplers[source_idx].sample_pixel_opt(
                source_idx,
                x1 as isize,
                y0 as isize,
                chunk_map,
            )?,
            tx * (1.0 - ty),
        ),
        (
            samplers[source_idx].sample_pixel_opt(
                source_idx,
                x0 as isize,
                y1 as isize,
                chunk_map,
            )?,
            (1.0 - tx) * ty,
        ),
        (
            samplers[source_idx].sample_pixel_opt(
                source_idx,
                x1 as isize,
                y1 as isize,
                chunk_map,
            )?,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resample::Pt;

    #[test]
    fn render_row_cursors_match_mapping_eval_for_every_pixel() {
        // Two sources with different cell counts: a curved one (refines) and an affine one (1 cell).
        let curved = |u: f64, v: f64| Pt {
            x: 100.0 + 512.0 * u + 30.0 * u * u - 5.0 * v,
            y: 7.0 + 512.0 * v + 40.0 * v * v + 3.0 * u,
        };
        let affine = |u: f64, v: f64| Pt {
            x: 1000.0 + 256.0 * u,
            y: 2000.0 + 256.0 * v,
        };
        let selected = vec![
            (
                3usize,
                TileMapping::refine(0.125, TILE_SIZE, |u, v| Ok(curved(u, v))).unwrap(),
            ),
            (
                7usize,
                TileMapping::refine(0.125, TILE_SIZE, |u, v| Ok(affine(u, v))).unwrap(),
            ),
        ];
        assert!(selected[0].1.cells() > 1);
        assert_eq!(selected[1].1.cells(), 1);
        let denom = (TILE_SIZE as f64 - 1.0).max(1.0);

        for j in [0usize, 1, 200, TILE_SIZE - 1] {
            let v = j as f64 / denom;
            let mut cursors = Vec::new();
            refill_row_cursors(&mut cursors, &selected, v, denom);
            let mut row = vec![0u8; TILE_SIZE * 4];
            let mut seen = Vec::new();
            render_row(&mut cursors, &selected, v, denom, &mut row, |cursors| {
                seen.push(cursors.to_vec());
                Ok([1, 2, 3, 4])
            })
            .unwrap();

            assert_eq!(seen.len(), TILE_SIZE);
            assert_eq!(row, [1u8, 2, 3, 4].repeat(TILE_SIZE));
            for (i, cursors) in seen.iter().enumerate() {
                let u = i as f64 / denom;
                for (c, (source_idx, mapping)) in cursors.iter().zip(&selected) {
                    assert_eq!(c.source_idx, *source_idx);
                    let want = mapping.eval(u, v);
                    assert!(
                        (c.x - want.x).abs() < 1e-6 && (c.y - want.y).abs() < 1e-6,
                        "row {j} px {i} source {source_idx}: cursor ({}, {}) vs eval ({}, {})",
                        c.x,
                        c.y,
                        want.x,
                        want.y
                    );
                }
            }
        }
    }
}
