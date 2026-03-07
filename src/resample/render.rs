use ravif::{AlphaColorMode, BitDepth, Encoder, Img, RGBA8};

use super::{Pt, TILE_SIZE};

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
        .with_alpha_color_mode(AlphaColorMode::UnassociatedClean)
        .with_num_threads(Some(1))
}

pub(crate) fn encode_avif(
    encoder: &Encoder<'_>,
    rgba: &[u8],
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let pixels: Vec<RGBA8> = rgba
        .chunks_exact(4)
        .map(|c| RGBA8::new(c[0], c[1], c[2], c[3]))
        .collect();
    let img = Img::new(&pixels[..], TILE_SIZE, TILE_SIZE);
    let encoded = encoder.encode_rgba(img)?;
    Ok(encoded.avif_file)
}
