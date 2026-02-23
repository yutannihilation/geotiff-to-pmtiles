use std::fs::File;
use std::io::BufReader;

use image::ImageReader;
use tiff::decoder::{ChunkType, Decoder, DecodingResult};
use tiff::tags::Tag;

use super::Raster;

pub(crate) fn load_raster(path: &std::path::Path) -> Result<Raster, Box<dyn std::error::Error>> {
    // Prefer `image` crate decoding for proper RGB/RGBA output when TIFF has multiple bands.
    if let Ok(reader) = ImageReader::open(path)
        && let Ok(dynamic) = reader.decode()
    {
        let rgba = dynamic.to_rgba8();
        return Ok(Raster {
            width: rgba.width() as usize,
            height: rgba.height() as usize,
            stride: 4,
            data: rgba.into_raw(),
        });
    }

    // Fallback: decode via `tiff` crate for cases `image` cannot decode.
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
        // Planar TIFF stores each band in separate chunks. Re-interleave to pixel-major layout.
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
    let stride = if derived_stride == 0 {
        samples_tag.max(1)
    } else {
        derived_stride
    };

    Ok(Raster {
        width,
        height,
        stride,
        data,
    })
}

pub(crate) fn decoding_result_to_u8(image: DecodingResult) -> Vec<u8> {
    // Normalize all numeric TIFF sample formats to a common u8 channel representation.
    match image {
        DecodingResult::U8(v) => v,
        DecodingResult::U16(v) => {
            normalize_slice_to_u8(&v, u16::MIN as f64, u16::MAX as f64, |x| *x as f64)
        }
        DecodingResult::U32(v) => {
            normalize_slice_to_u8(&v, u32::MIN as f64, u32::MAX as f64, |x| *x as f64)
        }
        DecodingResult::U64(v) => normalize_slice_to_u8(&v, 0.0, u64::MAX as f64, |x| *x as f64),
        DecodingResult::I8(v) => {
            normalize_slice_to_u8(&v, i8::MIN as f64, i8::MAX as f64, |x| *x as f64)
        }
        DecodingResult::I16(v) => {
            normalize_slice_to_u8(&v, i16::MIN as f64, i16::MAX as f64, |x| *x as f64)
        }
        DecodingResult::I32(v) => {
            normalize_slice_to_u8(&v, i32::MIN as f64, i32::MAX as f64, |x| *x as f64)
        }
        DecodingResult::I64(v) => {
            normalize_slice_to_u8(&v, i64::MIN as f64, i64::MAX as f64, |x| *x as f64)
        }
        DecodingResult::F32(v) => {
            normalize_slice_to_u8(&v, f32::MIN as f64, f32::MAX as f64, |x| *x as f64)
        }
        DecodingResult::F16(v) => {
            normalize_slice_to_u8(&v, f32::MIN as f64, f32::MAX as f64, |x| x.to_f32() as f64)
        }
        DecodingResult::F64(v) => normalize_slice_to_u8(&v, f64::MIN, f64::MAX, |x| *x),
    }
}

fn normalize_slice_to_u8<T, F>(
    values: &[T],
    fallback_min: f64,
    fallback_max: f64,
    to_f64: F,
) -> Vec<u8>
where
    F: Fn(&T) -> f64 + Copy,
{
    // Two-pass normalization over the original typed slice. This avoids
    // allocating a temporary `Vec<f64>` for large rasters.
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    for v in values {
        let n = to_f64(v);
        if n.is_finite() {
            min = min.min(n);
            max = max.max(n);
        }
    }

    // If data is constant or non-finite, fallback to the type range to avoid division by zero.
    if !min.is_finite() || !max.is_finite() || (max - min).abs() < f64::EPSILON {
        min = fallback_min;
        max = fallback_max;
    }

    let range = (max - min).abs();
    if range < f64::EPSILON {
        return vec![0; values.len()];
    }

    values
        .iter()
        .map(|v| {
            let t = ((to_f64(v) - min) / range).clamp(0.0, 1.0);
            (t * 255.0).round() as u8
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoding_u8_is_passthrough() {
        let out = decoding_result_to_u8(DecodingResult::U8(vec![1, 2, 3]));
        assert_eq!(out, vec![1, 2, 3]);
    }

    #[test]
    fn decoding_u16_normalizes_to_full_range() {
        let out = decoding_result_to_u8(DecodingResult::U16(vec![0, 65_535]));
        assert_eq!(out, vec![0, 255]);
    }

    #[test]
    fn decoding_i16_normalizes_min_to_max() {
        let out = decoding_result_to_u8(DecodingResult::I16(vec![-10, 0, 10]));
        assert_eq!(out, vec![0, 128, 255]);
    }

    #[test]
    fn decoding_constant_values_falls_back_safely() {
        let out = decoding_result_to_u8(DecodingResult::U16(vec![5, 5, 5]));
        assert_eq!(out, vec![0, 0, 0]);
    }

    #[test]
    fn decoding_non_finite_f32_values_is_stable() {
        let out = decoding_result_to_u8(DecodingResult::F32(vec![f32::NAN, 0.0, f32::INFINITY]));
        assert_eq!(out, vec![0, 128, 255]);
    }
}
