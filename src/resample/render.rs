use std::fs::File;

use image::ExtendedColorType;
use image::ImageEncoder;
use image::codecs::avif::AvifEncoder;
use rayon::prelude::*;

use crate::cli::Resampling;

use super::{NoDataSpec, Pt, Raster};

pub(crate) fn render_tile_debug_multi(
    sources: &[(&Raster, [Pt; 4])],
    resampling: Resampling,
    nodata: Option<NoDataSpec>,
) -> Vec<u8> {
    const SIZE: usize = 512;
    let mut out = vec![0_u8; SIZE * SIZE * 4];

    out.par_chunks_mut(SIZE * 4)
        .enumerate()
        .for_each(|(j, row)| {
            let v = if SIZE > 1 {
                j as f64 / (SIZE as f64 - 1.0)
            } else {
                0.0
            };

            for i in 0..SIZE {
                let u = if SIZE > 1 {
                    i as f64 / (SIZE as f64 - 1.0)
                } else {
                    0.0
                };
                let rgba = match resampling {
                    Resampling::Nearest => {
                        // Nearest across many sources:
                        // 1. Project output pixel to each source raster via bilinear corner interpolation.
                        // 2. Sample nearest pixel in that source.
                        // 3. Keep globally nearest candidate by pixel-space distance.
                        let mut best: Option<([u8; 4], f64)> = None;
                        for (raster, corners) in sources {
                            let left = lerp(corners[0], corners[3], v);
                            let right = lerp(corners[1], corners[2], v);
                            let p = lerp(left, right, u);
                            if let Some((px, dist2)) =
                                sample_nearest_with_dist(raster, p.x, p.y, nodata)
                            {
                                match best {
                                    Some((_, d)) if d <= dist2 => {}
                                    _ => best = Some((px, dist2)),
                                }
                            }
                        }
                        best.map(|(px, _)| px).unwrap_or([0, 0, 0, 0])
                    }
                    Resampling::Bilinear => {
                        // Bilinear across many sources (simple policy):
                        // use the first source in input order that can produce a valid sample.
                        let mut chosen = None;
                        for (raster, corners) in sources {
                            let left = lerp(corners[0], corners[3], v);
                            let right = lerp(corners[1], corners[2], v);
                            let p = lerp(left, right, u);
                            if let Some(px) = sample_bilinear_opt(raster, p.x, p.y, nodata) {
                                chosen = Some(px);
                                break;
                            }
                        }
                        chosen.unwrap_or([0, 0, 0, 0])
                    }
                };

                let base = i * 4;
                row[base] = rgba[0];
                row[base + 1] = rgba[1];
                row[base + 2] = rgba[2];
                row[base + 3] = rgba[3];
            }
        });

    out
}

fn sample_nearest_with_dist(
    raster: &Raster,
    x: f64,
    y: f64,
    nodata: Option<NoDataSpec>,
) -> Option<([u8; 4], f64)> {
    let x0 = x.floor() as isize;
    let y0 = y.floor() as isize;
    // Candidate set from the containing 2x2 cell. We sort by Euclidean distance and
    // return the first in-bounds, non-nodata sample.
    let mut candidates = [(x0, y0), (x0 + 1, y0), (x0, y0 + 1), (x0 + 1, y0 + 1)];
    candidates.sort_by(|(ax, ay), (bx, by)| {
        let da = (*ax as f64 - x).powi(2) + (*ay as f64 - y).powi(2);
        let db = (*bx as f64 - x).powi(2) + (*by as f64 - y).powi(2);
        da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
    });

    for (xi, yi) in candidates {
        let Some(px) = sample_pixel_opt(raster, xi, yi) else {
            continue;
        };
        if let Some(nd) = nodata {
            if nd.is_nodata(px) {
                continue;
            }
        }
        let dist2 = (xi as f64 - x).powi(2) + (yi as f64 - y).powi(2);
        return Some((px, dist2));
    }

    None
}

fn sample_bilinear_opt(
    raster: &Raster,
    x: f64,
    y: f64,
    nodata: Option<NoDataSpec>,
) -> Option<[u8; 4]> {
    let x0 = x.floor();
    let y0 = y.floor();
    let x1 = x0 + 1.0;
    let y1 = y0 + 1.0;

    let tx = x - x0;
    let ty = y - y0;

    // Standard 2x2 bilinear weights at (x, y), but invalid/nodata neighbors are skipped.
    let samples = [
        (
            sample_pixel_opt(raster, x0 as isize, y0 as isize),
            (1.0 - tx) * (1.0 - ty),
        ),
        (
            sample_pixel_opt(raster, x1 as isize, y0 as isize),
            tx * (1.0 - ty),
        ),
        (
            sample_pixel_opt(raster, x0 as isize, y1 as isize),
            (1.0 - tx) * ty,
        ),
        (sample_pixel_opt(raster, x1 as isize, y1 as isize), tx * ty),
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

    // If all neighbors are invalid/nodata, report transparent.
    if wsum <= f64::EPSILON {
        return None;
    }

    let mut out = [0_u8; 4];
    for c in 0..4 {
        out[c] = (acc[c] / wsum).round().clamp(0.0, 255.0) as u8;
    }
    Some(out)
}

fn sample_pixel_opt(raster: &Raster, xi: isize, yi: isize) -> Option<[u8; 4]> {
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

pub(crate) fn lerp(a: Pt, b: Pt, t: f64) -> Pt {
    Pt {
        x: a.x + (b.x - a.x) * t,
        y: a.y + (b.y - a.y) * t,
    }
}

pub(crate) fn write_avif(path: &str, rgba: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let file = File::create(path)?;
    let encoder = AvifEncoder::new(file);
    encoder.write_image(rgba, 512, 512, ExtendedColorType::Rgba8)?;
    Ok(())
}

pub(crate) fn encode_avif(rgba: &[u8]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut out = Vec::new();
    let encoder = AvifEncoder::new(&mut out);
    encoder.write_image(rgba, 512, 512, ExtendedColorType::Rgba8)?;
    Ok(out)
}
