use ravif::{BitDepth, Encoder, Img, RGBA8};

use super::{Pt, TILE_SIZE};

pub(crate) fn encode_png(
    rgba: &[u8],
    compression: png::Compression,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut buf = Vec::with_capacity(TILE_SIZE * TILE_SIZE * 4);
    {
        let mut encoder = png::Encoder::new(&mut buf, TILE_SIZE as u32, TILE_SIZE as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(compression);
        let mut writer = encoder.write_header()?;
        writer.write_image_data(rgba)?;
    }
    Ok(buf)
}

pub(crate) fn lerp(a: Pt, b: Pt, t: f64) -> Pt {
    Pt {
        x: a.x + (b.x - a.x) * t,
        y: a.y + (b.y - a.y) * t,
    }
}

pub(crate) fn make_avif_encoder(speed: u8, quality: u8) -> Encoder<'static> {
    Encoder::new()
        .with_quality(quality as f32)
        .with_alpha_quality(quality as f32)
        .with_speed(speed)
        .with_bit_depth(BitDepth::Eight)
}

pub(crate) fn encode_avif(
    encoder: &Encoder<'_>,
    rgba: &[u8],
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let pixels: &[RGBA8] = rgb::bytemuck::cast_slice(rgba);
    let img = Img::new(pixels, TILE_SIZE, TILE_SIZE);
    let encoded = encoder.encode_rgba(img)?;
    Ok(encoded.avif_file)
}
