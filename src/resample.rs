use std::collections::BTreeSet;
use std::fs::File;
use std::io::BufReader;

use crate::cli::Resampling;
use image::ExtendedColorType;
use image::ImageEncoder;
use image::ImageReader;
use image::codecs::avif::AvifEncoder;
use proj_lite::Proj;
use rayon::prelude::*;
use tiff::decoder::{ChunkType, Decoder, DecodingResult};
use tiff::tags::Tag;

#[derive(Debug, Clone, Copy)]
struct Pt {
    x: f64,
    y: f64,
}

#[derive(Debug, Clone, Copy)]
enum GeoTransform {
    TiePointAndPixelScale {
        raster_x: f64,
        raster_y: f64,
        model_x: f64,
        model_y: f64,
        scale_x: f64,
        scale_y: f64,
    },
    Affine {
        t0: f64,
        t1: f64,
        t2: f64,
        t3: f64,
        t4: f64,
        t5: f64,
    },
}

impl GeoTransform {
    fn apply(self, p: Pt) -> Pt {
        match self {
            GeoTransform::TiePointAndPixelScale {
                raster_x,
                raster_y,
                model_x,
                model_y,
                scale_x,
                scale_y,
            } => Pt {
                x: (p.x - raster_x) * scale_x + model_x,
                y: (p.y - raster_y) * -scale_y + model_y,
            },
            GeoTransform::Affine {
                t0,
                t1,
                t2,
                t3,
                t4,
                t5,
            } => Pt {
                x: p.x * t0 + p.y * t1 + t2,
                y: p.x * t3 + p.y * t4 + t5,
            },
        }
    }

    fn invert(self) -> Result<Self, Box<dyn std::error::Error>> {
        match self {
            GeoTransform::TiePointAndPixelScale {
                raster_x,
                raster_y,
                model_x,
                model_y,
                scale_x,
                scale_y,
            } => {
                if scale_x == 0.0 || scale_y == 0.0 {
                    return Err("invalid tie-point transform: zero scale".into());
                }
                Ok(GeoTransform::TiePointAndPixelScale {
                    raster_x: model_x,
                    raster_y: model_y,
                    model_x: raster_x,
                    model_y: raster_y,
                    scale_x: 1.0 / scale_x,
                    scale_y: 1.0 / scale_y,
                })
            }
            GeoTransform::Affine {
                t0,
                t1,
                t2,
                t3,
                t4,
                t5,
            } => {
                let det = t0 * t4 - t1 * t3;
                if det.abs() < 1e-15 {
                    return Err("affine transform is not invertible".into());
                }
                Ok(GeoTransform::Affine {
                    t0: t4 / det,
                    t1: -t1 / det,
                    t2: (t1 * t5 - t2 * t4) / det,
                    t3: -t3 / det,
                    t4: t0 / det,
                    t5: (-t0 * t5 + t2 * t3) / det,
                })
            }
        }
    }
}

struct Raster {
    width: usize,
    height: usize,
    stride: usize,
    data: Vec<u8>,
}

pub fn resample_tiles(
    path: &std::path::Path,
    src_crs: Option<&str>,
    resampling: Resampling,
) -> Result<(), Box<dyn std::error::Error>> {
    let raster = load_raster(path)?;
    let georef = read_georef(path, src_crs)?;

    let corners_px = [
        Pt {
            x: georef.raster_offset,
            y: georef.raster_offset,
        },
        Pt {
            x: raster.width as f64 + georef.raster_offset,
            y: georef.raster_offset,
        },
        Pt {
            x: raster.width as f64 + georef.raster_offset,
            y: raster.height as f64 + georef.raster_offset,
        },
        Pt {
            x: georef.raster_offset,
            y: raster.height as f64 + georef.raster_offset,
        },
    ];

    let corners_src = corners_px.map(|p| georef.forward.apply(p));
    let to_merc = Proj::new_known_crs(&georef.source_crs, "EPSG:3857")?;
    let corners_merc = corners_src
        .map(|p| to_merc.transform2((p.x, p.y)).map(|(x, y)| Pt { x, y }))
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;

    let largest_edge = largest_edge_length(&corners_merc)?;
    let z = zoom_for_tile_size(largest_edge);

    let mut tiles = BTreeSet::new();
    for corner in &corners_merc {
        let (x, y) = webmerc_to_tile(corner.x, corner.y, z);
        tiles.insert((x, y));
    }

    let from_merc = Proj::new_known_crs("EPSG:3857", &georef.source_crs)?;
    let inverse = georef.forward.invert()?;

    println!("File: {}", path.display());
    println!("Source CRS: {}", georef.source_crs);
    println!(
        "Transform source: {}",
        if georef.used_tfw {
            "world file (.tfw)"
        } else {
            "GeoTIFF tags"
        }
    );
    println!("Selected zoom: {z}");
    println!("Output tiles: {}", tiles.len());

    for (idx, (x, y)) in tiles.iter().enumerate() {
        let bounds = tile_bounds_webmerc(z, *x, *y);

        let tile_merc_corners = [bounds.ul, bounds.ur, bounds.lr, bounds.ll];
        let mut tile_raster_corners = [Pt { x: 0.0, y: 0.0 }; 4];
        for (i, p) in tile_merc_corners.iter().enumerate() {
            let (sx, sy) = from_merc.transform2((p.x, p.y))?;
            tile_raster_corners[i] = inverse.apply(Pt { x: sx, y: sy });
        }

        let out = render_tile_debug(&raster, tile_raster_corners, resampling);
        let filename = format!("out{}.avif", idx + 1);
        write_avif(&filename, &out)?;

        println!("  wrote {} for z={}, x={}, y={}", filename, z, x, y);
    }

    Ok(())
}

fn render_tile_debug(raster: &Raster, corners: [Pt; 4], resampling: Resampling) -> Vec<u8> {
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
            let left = lerp(corners[0], corners[3], v);
            let right = lerp(corners[1], corners[2], v);

            for i in 0..SIZE {
                let u = if SIZE > 1 {
                    i as f64 / (SIZE as f64 - 1.0)
                } else {
                    0.0
                };
                let p = lerp(left, right, u);
                let rgba = match resampling {
                    Resampling::Nearest => sample_nearest(raster, p.x, p.y),
                    Resampling::Bilinear => sample_bilinear(raster, p.x, p.y),
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

fn sample_nearest(raster: &Raster, x: f64, y: f64) -> [u8; 4] {
    let xi = x.round() as isize;
    let yi = y.round() as isize;

    if xi < 0 || yi < 0 || xi >= raster.width as isize || yi >= raster.height as isize {
        return [0, 0, 0, 0];
    }

    let xi = xi as usize;
    let yi = yi as usize;
    let pixel_index = yi * raster.width + xi;
    let base = pixel_index.saturating_mul(raster.stride);
    if base >= raster.data.len() {
        return [0, 0, 0, 0];
    }

    match raster.stride {
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
    }
}

fn sample_bilinear(raster: &Raster, x: f64, y: f64) -> [u8; 4] {
    let x0 = x.floor();
    let y0 = y.floor();
    let x1 = x0 + 1.0;
    let y1 = y0 + 1.0;

    let tx = x - x0;
    let ty = y - y0;

    let p00 = sample_nearest(raster, x0, y0);
    let p10 = sample_nearest(raster, x1, y0);
    let p01 = sample_nearest(raster, x0, y1);
    let p11 = sample_nearest(raster, x1, y1);

    let mut out = [0_u8; 4];
    for c in 0..4 {
        let v00 = p00[c] as f64;
        let v10 = p10[c] as f64;
        let v01 = p01[c] as f64;
        let v11 = p11[c] as f64;

        let top = v00 + (v10 - v00) * tx;
        let bottom = v01 + (v11 - v01) * tx;
        let v = top + (bottom - top) * ty;
        out[c] = v.round().clamp(0.0, 255.0) as u8;
    }
    out
}

fn lerp(a: Pt, b: Pt, t: f64) -> Pt {
    Pt {
        x: a.x + (b.x - a.x) * t,
        y: a.y + (b.y - a.y) * t,
    }
}

fn write_avif(path: &str, rgba: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let file = File::create(path)?;
    let encoder = AvifEncoder::new(file);
    encoder.write_image(rgba, 512, 512, ExtendedColorType::Rgba8)?;
    Ok(())
}

struct Georef {
    source_crs: String,
    forward: GeoTransform,
    raster_offset: f64,
    used_tfw: bool,
}

fn read_georef(path: &std::path::Path, src_crs: Option<&str>) -> Result<Georef, Box<dyn std::error::Error>> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut decoder = Decoder::new(reader)?;

    let geokey_directory = decoder
        .find_tag(Tag::GeoKeyDirectoryTag)?
        .map(|value| value.into_u16_vec())
        .transpose()?;
    let (source_crs, raster_type) = if let Some(geokey_directory) = geokey_directory.as_deref() {
        (
            format!("EPSG:{}", source_epsg_from_geokeys(geokey_directory)?),
            geokey_short(geokey_directory, 1025),
        )
    } else {
        let src_crs = src_crs.ok_or(
            "GeoKeyDirectoryTag is missing. Provide --src-crs (e.g. --src-crs EPSG:4326).",
        )?;
        (src_crs.to_string(), None)
    };

    let mut used_tfw = false;
    let forward = if let Some(matrix) = decoder
        .find_tag(Tag::ModelTransformationTag)?
        .map(|value| value.into_f64_vec())
        .transpose()?
    {
        if matrix.len() < 16 {
            return Err("ModelTransformationTag must contain 16 values".into());
        }
        GeoTransform::Affine {
            t0: matrix[0],
            t1: matrix[1],
            t2: matrix[3],
            t3: matrix[4],
            t4: matrix[5],
            t5: matrix[7],
        }
    } else if let (Some(pixel_scale), Some(tie_points)) = (
        decoder
            .find_tag(Tag::ModelPixelScaleTag)?
            .map(|value| value.into_f64_vec())
            .transpose()?,
        decoder
            .find_tag(Tag::ModelTiepointTag)?
            .map(|value| value.into_f64_vec())
            .transpose()?,
    ) {
        if pixel_scale.len() < 2 {
            return Err("ModelPixelScaleTag must contain at least two values".into());
        }
        if tie_points.len() < 6 {
            return Err("ModelTiepointTag must contain at least one tie point".into());
        }
        GeoTransform::TiePointAndPixelScale {
            raster_x: tie_points[0],
            raster_y: tie_points[1],
            model_x: tie_points[3],
            model_y: tie_points[4],
            scale_x: pixel_scale[0],
            scale_y: pixel_scale[1],
        }
    } else if let Some(tfw_transform) = read_world_file_transform(path)? {
        used_tfw = true;
        tfw_transform
    } else {
        return Err(
            "No georeferencing transform found. Expected ModelTransformationTag, ModelPixelScaleTag+ModelTiepointTag, or adjacent .tfw file."
                .into(),
        );
    };

    let raster_offset = if used_tfw {
        -0.5
    } else if raster_type == Some(2) {
        -0.5
    } else {
        0.0
    };

    Ok(Georef {
        source_crs,
        forward,
        raster_offset,
        used_tfw,
    })
}

fn load_raster(path: &std::path::Path) -> Result<Raster, Box<dyn std::error::Error>> {
    // Prefer `image` crate decoding for proper RGB/RGBA output when TIFF has multiple bands.
    if let Ok(reader) = ImageReader::open(path) {
        if let Ok(dynamic) = reader.decode() {
            let rgba = dynamic.to_rgba8();
            return Ok(Raster {
                width: rgba.width() as usize,
                height: rgba.height() as usize,
                stride: 4,
                data: rgba.into_raw(),
            });
        }
    }

    // Fallback: decode via `tiff` crate.
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut decoder = Decoder::new(reader)?;

    let (width_u32, height_u32) = decoder.dimensions()?;
    let width = width_u32 as usize;
    let height = height_u32 as usize;
    let samples_tag = decoder
        .find_tag(Tag::SamplesPerPixel)?
        .map(|v| v.into_u16())
        .transpose()?
        .unwrap_or(1) as usize;
    let planar_config = decoder
        .find_tag(Tag::PlanarConfiguration)?
        .map(|v| v.into_u16())
        .transpose()?
        .unwrap_or(1);

    if planar_config == 2 && samples_tag > 1 {
        let chunk_type = decoder.get_chunk_type();
        let total_chunks = match chunk_type {
            ChunkType::Strip => decoder.strip_count()? as usize,
            ChunkType::Tile => decoder.tile_count()? as usize,
        };
        if total_chunks == 0 || total_chunks % samples_tag != 0 {
            return Err("invalid planar TIFF chunk layout".into());
        }
        let chunks_per_sample = total_chunks / samples_tag;
        let default_chunk_dims = decoder.chunk_dimensions();
        let chunks_across = width.div_ceil(default_chunk_dims.0 as usize);

        let mut data = vec![0_u8; width * height * samples_tag];
        for sample in 0..samples_tag {
            for c in 0..chunks_per_sample {
                let chunk_idx = (sample * chunks_per_sample + c) as u32;
                let (cw, ch) = decoder.chunk_data_dimensions(chunk_idx);
                let chunk = decoder.read_chunk(chunk_idx)?;
                let chunk_values = decoding_result_to_u8(chunk);
                let cw = cw as usize;
                let ch = ch as usize;

                let x0 = (c % chunks_across) * default_chunk_dims.0 as usize;
                let y0 = (c / chunks_across) * default_chunk_dims.1 as usize;

                for dy in 0..ch {
                    let yy = y0 + dy;
                    if yy >= height {
                        continue;
                    }
                    for dx in 0..cw {
                        let xx = x0 + dx;
                        if xx >= width {
                            continue;
                        }
                        let src = dy * cw + dx;
                        if src >= chunk_values.len() {
                            continue;
                        }
                        let dst = (yy * width + xx) * samples_tag + sample;
                        data[dst] = chunk_values[src];
                    }
                }
            }
        }

        return Ok(Raster {
            width,
            height,
            stride: samples_tag,
            data,
        });
    }

    let image = decoder.read_image()?;

    let data = decoding_result_to_u8(image);

    let pixel_count = width.saturating_mul(height);
    let derived_stride = if pixel_count == 0 {
        0
    } else {
        data.len() / pixel_count
    };
    let stride = if derived_stride == 0 { samples_tag.max(1) } else { derived_stride };

    Ok(Raster { width, height, stride, data })
}

fn decoding_result_to_u8(image: DecodingResult) -> Vec<u8> {
    match image {
        DecodingResult::U8(v) => v,
        DecodingResult::U16(v) => normalize_to_u8(
            v.iter().map(|x| *x as f64).collect::<Vec<_>>(),
            u16::MIN as f64,
            u16::MAX as f64,
        ),
        DecodingResult::U32(v) => normalize_to_u8(
            v.iter().map(|x| *x as f64).collect::<Vec<_>>(),
            u32::MIN as f64,
            u32::MAX as f64,
        ),
        DecodingResult::U64(v) => normalize_to_u8(
            v.iter().map(|x| *x as f64).collect::<Vec<_>>(),
            0.0,
            u64::MAX as f64,
        ),
        DecodingResult::I8(v) => normalize_to_u8(
            v.iter().map(|x| *x as f64).collect::<Vec<_>>(),
            i8::MIN as f64,
            i8::MAX as f64,
        ),
        DecodingResult::I16(v) => normalize_to_u8(
            v.iter().map(|x| *x as f64).collect::<Vec<_>>(),
            i16::MIN as f64,
            i16::MAX as f64,
        ),
        DecodingResult::I32(v) => normalize_to_u8(
            v.iter().map(|x| *x as f64).collect::<Vec<_>>(),
            i32::MIN as f64,
            i32::MAX as f64,
        ),
        DecodingResult::I64(v) => normalize_to_u8(
            v.iter().map(|x| *x as f64).collect::<Vec<_>>(),
            i64::MIN as f64,
            i64::MAX as f64,
        ),
        DecodingResult::F32(v) => normalize_to_u8(
            v.iter().map(|x| *x as f64).collect::<Vec<_>>(),
            f32::MIN as f64,
            f32::MAX as f64,
        ),
        DecodingResult::F64(v) => normalize_to_u8(v, f64::MIN, f64::MAX),
    }
}

fn normalize_to_u8(values: Vec<f64>, fallback_min: f64, fallback_max: f64) -> Vec<u8> {
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    for v in &values {
        if v.is_finite() {
            min = min.min(*v);
            max = max.max(*v);
        }
    }

    if !min.is_finite() || !max.is_finite() || (max - min).abs() < f64::EPSILON {
        min = fallback_min;
        max = fallback_max;
    }

    let range = (max - min).abs();
    if range < f64::EPSILON {
        return vec![0; values.len()];
    }

    values
        .into_iter()
        .map(|v| {
            let t = ((v - min) / range).clamp(0.0, 1.0);
            (t * 255.0).round() as u8
        })
        .collect()
}

fn read_world_file_transform(
    path: &std::path::Path,
) -> Result<Option<GeoTransform>, Box<dyn std::error::Error>> {
    let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
        return Ok(None);
    };
    let Some(parent) = path.parent() else {
        return Ok(None);
    };

    let candidates = [
        parent.join(format!("{stem}.tfw")),
        parent.join(format!("{stem}.TFW")),
        parent.join(format!("{stem}.tifw")),
        parent.join(format!("{stem}.TIFW")),
    ];

    let world_file = match candidates.into_iter().find(|p| p.is_file()) {
        Some(p) => p,
        None => return Ok(None),
    };

    let content = std::fs::read_to_string(&world_file)?;
    let values = content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::parse::<f64>)
        .collect::<Result<Vec<_>, _>>()?;

    if values.len() != 6 {
        return Err(format!(
            "invalid world file '{}': expected 6 numeric lines, got {}",
            world_file.display(),
            values.len()
        )
        .into());
    }

    let a = values[0];
    let d = values[1];
    let b = values[2];
    let e = values[3];
    let c = values[4];
    let f = values[5];

    Ok(Some(GeoTransform::Affine {
        t0: a,
        t1: b,
        t2: c,
        t3: d,
        t4: e,
        t5: f,
    }))
}

fn source_epsg_from_geokeys(data: &[u16]) -> Result<u16, Box<dyn std::error::Error>> {
    if data.len() < 4 {
        return Err("invalid GeoKeyDirectoryTag: too short".into());
    }
    let geographic = geokey_short(data, 2048);
    let projected = geokey_short(data, 3072);

    if let Some(code) = projected.filter(|code| *code != 0 && *code != 32767) {
        return Ok(code);
    }
    if let Some(code) = geographic.filter(|code| *code != 0 && *code != 32767) {
        return Ok(code);
    }

    Err("could not infer source EPSG from GeoKeyDirectoryTag".into())
}

fn geokey_short(data: &[u16], target_key: u16) -> Option<u16> {
    if data.len() < 4 {
        return None;
    }
    let key_count = data[3] as usize;
    for chunk in data[4..].chunks(4).take(key_count) {
        if chunk.len() != 4 {
            continue;
        }
        let key_id = chunk[0];
        let tiff_tag_location = chunk[1];
        let count = chunk[2];
        let value_or_offset = chunk[3];
        if key_id == target_key && tiff_tag_location == 0 && count == 1 {
            return Some(value_or_offset);
        }
    }
    None
}

fn largest_edge_length(corners: &[Pt]) -> Result<f64, Box<dyn std::error::Error>> {
    if corners.len() < 4 {
        return Err("expected 4 corners".into());
    }

    let d0 = distance(corners[0], corners[1]);
    let d1 = distance(corners[1], corners[2]);
    let d2 = distance(corners[2], corners[3]);
    let d3 = distance(corners[3], corners[0]);

    Ok(d0.max(d1).max(d2).max(d3))
}

fn distance(a: Pt, b: Pt) -> f64 {
    let dx = a.x - b.x;
    let dy = a.y - b.y;
    (dx * dx + dy * dy).sqrt()
}

fn zoom_for_tile_size(required_size: f64) -> u8 {
    const MAX_ZOOM: u8 = 24;
    const ORIGIN_SHIFT: f64 = 20_037_508.342_789_244;
    let world_size = 2.0 * ORIGIN_SHIFT;

    for z in (0..=MAX_ZOOM).rev() {
        let tile_size = world_size / (1_u32 << z) as f64;
        if tile_size >= required_size {
            return z;
        }
    }

    0
}

fn webmerc_to_tile(x: f64, y: f64, z: u8) -> (u32, u32) {
    const ORIGIN_SHIFT: f64 = 20_037_508.342_789_244;
    let n = (1_u32 << z) as f64;

    let x_norm = ((x + ORIGIN_SHIFT) / (2.0 * ORIGIN_SHIFT)).clamp(0.0, 1.0 - f64::EPSILON);
    let y_norm = ((ORIGIN_SHIFT - y) / (2.0 * ORIGIN_SHIFT)).clamp(0.0, 1.0 - f64::EPSILON);

    let xtile = (x_norm * n).floor() as u32;
    let ytile = (y_norm * n).floor() as u32;

    (xtile, ytile)
}

struct TileBounds {
    ul: Pt,
    ur: Pt,
    lr: Pt,
    ll: Pt,
}

fn tile_bounds_webmerc(z: u8, x: u32, y: u32) -> TileBounds {
    const ORIGIN_SHIFT: f64 = 20_037_508.342_789_244;
    let n = (1_u32 << z) as f64;
    let tile_size = (2.0 * ORIGIN_SHIFT) / n;

    let min_x = -ORIGIN_SHIFT + x as f64 * tile_size;
    let max_x = min_x + tile_size;
    let max_y = ORIGIN_SHIFT - y as f64 * tile_size;
    let min_y = max_y - tile_size;

    TileBounds {
        ul: Pt { x: min_x, y: max_y },
        ur: Pt { x: max_x, y: max_y },
        lr: Pt { x: max_x, y: min_y },
        ll: Pt { x: min_x, y: min_y },
    }
}
