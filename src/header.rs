use tiff_compio::tag;

pub fn dump_header(path: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    println!("File: {}", path.display());
    dump_header_tags(path)
}

fn dump_header_tags(path: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    let rt = compio::runtime::Runtime::new()?;
    let reader = rt.block_on(async {
        let file = compio::fs::File::open(path).await?;
        tiff_compio::TiffReader::new(file).await
    })?;

    let (width, height) = reader.dimensions()?;
    println!("Raster width: {width}");
    println!("Raster height: {height}");

    let samples_per_pixel = reader
        .find_tag(tag::SAMPLES_PER_PIXEL)
        .map(|v| v.into_u16())
        .transpose()?
        .unwrap_or(1);
    println!("Samples per pixel: {samples_per_pixel}");

    let bits_per_sample = reader
        .find_tag(tag::BITS_PER_SAMPLE)
        .map(|v| v.into_u16_vec())
        .transpose()?;
    println!(
        "Bits per sample: {}",
        bits_per_sample
            .map(|v| format!("{v:?}"))
            .unwrap_or_else(|| "n/a".to_string())
    );

    let photometric = reader
        .find_tag(tag::PHOTOMETRIC_INTERPRETATION)
        .map(|v| v.into_u16())
        .transpose()?
        .map(format_photometric);
    println!("Photometric interpretation: {}", format_opt(photometric));

    let compression = reader
        .find_tag(tag::COMPRESSION)
        .map(|v| v.into_u16())
        .transpose()?
        .map(format_compression);
    println!("Compression: {}", format_opt(compression));

    let sample_format = reader
        .find_tag(tag::SAMPLE_FORMAT)
        .map(|v| v.into_u16_vec())
        .transpose()?
        .map(|v| {
            v.into_iter()
                .map(format_sample_format)
                .collect::<Vec<_>>()
        });
    println!(
        "Sample format: {}",
        sample_format
            .map(|v| format!("{v:?}"))
            .unwrap_or_else(|| "n/a".to_string())
    );

    let has_geo_keys = reader.find_tag(tag::GEO_KEY_DIRECTORY).is_some();
    let has_pixel_scale = reader.find_tag(tag::MODEL_PIXEL_SCALE).is_some();
    let has_tiepoints = reader.find_tag(tag::MODEL_TIEPOINT).is_some();
    let has_transform = reader.find_tag(tag::MODEL_TRANSFORMATION).is_some();

    println!("Has GeoKeyDirectoryTag: {has_geo_keys}");
    println!("Has ModelPixelScaleTag: {has_pixel_scale}");
    println!("Has ModelTiepointTag: {has_tiepoints}");
    println!("Has ModelTransformationTag: {has_transform}");

    Ok(())
}

fn format_photometric(v: u16) -> String {
    match v {
        0 => "WhiteIsZero".to_string(),
        1 => "BlackIsZero".to_string(),
        2 => "RGB".to_string(),
        3 => "RGBPalette".to_string(),
        4 => "TransparencyMask".to_string(),
        5 => "CMYK".to_string(),
        6 => "YCbCr".to_string(),
        8 => "CIELab".to_string(),
        _ => format!("Unknown({v})"),
    }
}

fn format_compression(v: u16) -> String {
    match v {
        1 => "None".to_string(),
        2 => "CCITT".to_string(),
        3 => "Fax3".to_string(),
        4 => "Fax4".to_string(),
        5 => "LZW".to_string(),
        6 => "OldJpeg".to_string(),
        7 => "JPEG".to_string(),
        8 => "Deflate".to_string(),
        32946 => "OldDeflate".to_string(),
        34712 => "JPEG2000".to_string(),
        _ => format!("Unknown({v})"),
    }
}

fn format_sample_format(v: u16) -> String {
    match v {
        1 => "Uint".to_string(),
        2 => "Int".to_string(),
        3 => "IEEEFP".to_string(),
        4 => "Void".to_string(),
        _ => format!("Unknown({v})"),
    }
}

fn format_opt<T: std::fmt::Display>(value: Option<T>) -> String {
    match value {
        Some(value) => value.to_string(),
        None => "n/a".to_string(),
    }
}
