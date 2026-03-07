use image::ExtendedColorType;
use image::ImageEncoder;
use image::codecs::avif::AvifEncoder;

use super::{Pt, TILE_SIZE};

pub(crate) fn lerp(a: Pt, b: Pt, t: f64) -> Pt {
    Pt {
        x: a.x + (b.x - a.x) * t,
        y: a.y + (b.y - a.y) * t,
    }
}

pub(crate) fn encode_avif(
    rgba: &[u8],
    avif_speed: u8,
    avif_quality: u8,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut out = Vec::new();
    let encoder = AvifEncoder::new_with_speed_quality(&mut out, avif_speed, avif_quality);
    let tile_size = u32::try_from(TILE_SIZE)?;
    encoder.write_image(rgba, tile_size, tile_size, ExtendedColorType::Rgba8)?;
    Ok(out)
}
