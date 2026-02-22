use std::fs::File;
use std::io::BufReader;

use tiff::decoder::Decoder;
use tiff::tags::{CompressionMethod, PhotometricInterpretation, SampleFormat, Tag};

pub fn dump_header(path: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    println!("File: {}", path.display());
    dump_header_tags(path)
}

fn dump_header_tags(path: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut decoder = Decoder::new(reader)?;

    let (width, height) = decoder.dimensions()?;
    println!("Raster width: {width}");
    println!("Raster height: {height}");

    let samples_per_pixel = get_u16_tag(&mut decoder, Tag::SamplesPerPixel)?.unwrap_or(1);
    println!("Samples per pixel: {samples_per_pixel}");

    let bits_per_sample = decoder
        .find_tag(Tag::BitsPerSample)?
        .map(|value| value.into_u16_vec())
        .transpose()?;
    println!(
        "Bits per sample: {}",
        bits_per_sample
            .map(|v| format!("{v:?}"))
            .unwrap_or_else(|| "n/a".to_string())
    );

    let photometric = get_u16_tag(&mut decoder, Tag::PhotometricInterpretation)?.map(|v| {
        match PhotometricInterpretation::from_u16(v) {
            Some(value) => format!("{value:?}"),
            None => format!("Unknown({v})"),
        }
    });
    println!("Photometric interpretation: {}", format_opt(photometric));

    let compression =
        get_u16_tag(&mut decoder, Tag::Compression)?.map(CompressionMethod::from_u16_exhaustive);
    println!("Compression: {}", format_opt_debug(compression));

    let sample_format = decoder
        .find_tag(Tag::SampleFormat)?
        .map(|value| value.into_u16_vec())
        .transpose()?
        .map(|v| {
            v.into_iter()
                .map(SampleFormat::from_u16_exhaustive)
                .collect::<Vec<_>>()
        });
    println!("Sample format: {}", format_opt_debug(sample_format));

    let has_geo_keys = decoder.find_tag(Tag::GeoKeyDirectoryTag)?.is_some();
    let has_pixel_scale = decoder.find_tag(Tag::ModelPixelScaleTag)?.is_some();
    let has_tiepoints = decoder.find_tag(Tag::ModelTiepointTag)?.is_some();
    let has_transform = decoder.find_tag(Tag::ModelTransformationTag)?.is_some();

    println!("Has GeoKeyDirectoryTag: {has_geo_keys}");
    println!("Has ModelPixelScaleTag: {has_pixel_scale}");
    println!("Has ModelTiepointTag: {has_tiepoints}");
    println!("Has ModelTransformationTag: {has_transform}");

    Ok(())
}

fn get_u16_tag<R: std::io::Read + std::io::Seek>(
    decoder: &mut Decoder<R>,
    tag: Tag,
) -> Result<Option<u16>, tiff::TiffError> {
    decoder.find_tag(tag)?.map(|v| v.into_u16()).transpose()
}

fn format_opt<T: std::fmt::Display>(value: Option<T>) -> String {
    match value {
        Some(value) => value.to_string(),
        None => "n/a".to_string(),
    }
}

fn format_opt_debug<T: std::fmt::Debug>(value: Option<T>) -> String {
    match value {
        Some(value) => format!("{value:?}"),
        None => "n/a".to_string(),
    }
}
