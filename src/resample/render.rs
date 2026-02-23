use image::ExtendedColorType;
use image::ImageEncoder;
use image::codecs::avif::AvifEncoder;

use super::Pt;

pub(crate) fn lerp(a: Pt, b: Pt, t: f64) -> Pt {
    Pt {
        x: a.x + (b.x - a.x) * t,
        y: a.y + (b.y - a.y) * t,
    }
}

pub(crate) fn encode_avif(rgba: &[u8]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut out = Vec::new();
    let encoder = AvifEncoder::new(&mut out);
    encoder.write_image(rgba, 512, 512, ExtendedColorType::Rgba8)?;
    Ok(out)
}
