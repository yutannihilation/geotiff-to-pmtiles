use image::ImageReader;
use tiff_compio::tag;

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

    // Fallback: decode via tiff-compio for cases `image` cannot decode.
    let rt = compio::runtime::Runtime::new()?;
    let reader = rt.block_on(async {
        let file = compio::fs::File::open(path).await?;
        tiff_compio::TiffReader::new(file).await
    })?;
    let layout = reader.chunk_layout()?;

    let (width_u32, height_u32) = reader.dimensions()?;
    let width = width_u32 as usize;
    let height = height_u32 as usize;
    let samples_tag = reader
        .find_tag(tag::SAMPLES_PER_PIXEL)
        .map(|v: tiff_compio::TagValue| v.into_u16())
        .transpose()?
        .unwrap_or(1) as usize;
    let planar_config = reader
        .find_tag(tag::PLANAR_CONFIGURATION)
        .map(|v: tiff_compio::TagValue| v.into_u16())
        .transpose()?
        .unwrap_or(1);

    let bits_per_sample = layout.bits_per_sample[0];
    let sample_format = layout.sample_format;

    if planar_config == 2 && samples_tag > 1 {
        // Planar TIFF stores each band in separate chunks. Re-interleave to pixel-major layout.
        let total_chunks = layout.chunk_count as usize;
        if total_chunks == 0 || !total_chunks.is_multiple_of(samples_tag) {
            return Err("invalid planar TIFF chunk layout".into());
        }
        let chunks_per_sample = total_chunks / samples_tag;
        let chunks_across = layout.chunks_across as usize;

        let mut data = vec![0_u8; width * height * samples_tag];
        for sample in 0..samples_tag {
            for c in 0..chunks_per_sample {
                let chunk_idx = (sample * chunks_per_sample + c) as u32;
                let (cw, ch) = layout.chunk_data_dimensions(chunk_idx);
                let chunk_raw = rt.block_on(reader.read_chunk(&layout, chunk_idx))?;
                let chunk_values = tiff_compio::normalize::normalize_to_u8(
                    chunk_raw,
                    bits_per_sample,
                    sample_format,
                    reader.byte_order(),
                );
                let cw = cw as usize;
                let ch = ch as usize;

                let x0 = (c % chunks_across) * layout.chunk_width as usize;
                let y0 = (c / chunks_across) * layout.chunk_height as usize;

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

    let image_raw = rt.block_on(reader.read_image(&layout))?;
    let data = tiff_compio::normalize::normalize_to_u8(
        image_raw,
        bits_per_sample,
        sample_format,
        reader.byte_order(),
    );

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
