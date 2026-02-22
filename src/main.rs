use std::collections::BTreeSet;
use std::fs::File;
use std::io::BufReader;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use geotiff::GeoTiff;
use proj_lite::Proj;
use tiff::decoder::Decoder;
use tiff::tags::{CompressionMethod, PhotometricInterpretation, SampleFormat, Tag};

#[derive(Debug, Parser)]
#[command(name = "geotiff-to-pmtiles")]
#[command(about = "Utilities for working with GeoTIFF and PMTiles")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Print TIFF/GeoTIFF header information.
    DumpHeader {
        /// Path to a GeoTIFF file.
        input: std::path::PathBuf,
    },
    /// Compute the minimum covering Web Mercator Z/X/Y tile for a GeoTIFF.
    CoverTile {
        /// Path to a GeoTIFF file.
        input: std::path::PathBuf,
        /// Source CRS when GeoKeyDirectoryTag is missing (e.g. "EPSG:4326").
        #[arg(long)]
        src_crs: Option<String>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::DumpHeader { input } => dump_header(&input),
        Commands::CoverTile { input, src_crs } => cover_tile(&input, src_crs.as_deref()),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

fn dump_header(path: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    println!("File: {}", path.display());

    let file = File::open(path)?;
    let reader = BufReader::new(file);

    match GeoTiff::read(reader) {
        Ok(geotiff) => {
            let extent = geotiff.model_extent();
            let keys = &geotiff.geo_key_directory;

            println!("Raster width: {}", geotiff.raster_width);
            println!("Raster height: {}", geotiff.raster_height);
            println!("Samples per pixel: {}", geotiff.num_samples);
            println!(
                "Model extent: min=({}, {}), max=({}, {})",
                extent.min().x,
                extent.min().y,
                extent.max().x,
                extent.max().y
            );
            println!("GeoKeyDirectory version: {}", keys.key_directory_version);
            println!("GeoKey revision: {}.{}", keys.key_revision, keys.minor_revision);
            println!("Model type: {}", format_opt(keys.model_type));
            println!("Raster type: {}", format_opt_debug(keys.raster_type));
            println!("Geographic type (EPSG): {}", format_opt(keys.geographic_type));
            println!("Projected type (EPSG): {}", format_opt(keys.projected_type));
            println!("Geographic citation: {}", format_opt(keys.geog_citation.as_deref()));
            println!("Projection citation: {}", format_opt(keys.proj_citation.as_deref()));
        }
        Err(err) => {
            println!("GeoTIFF full decode: unsupported ({err})");
            println!("Falling back to tag-only header dump.");
            dump_header_tags(path)?;
        }
    }

    Ok(())
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

    let photometric = get_u16_tag(&mut decoder, Tag::PhotometricInterpretation)?
        .map(|v| match PhotometricInterpretation::from_u16(v) {
            Some(value) => format!("{value:?}"),
            None => format!("Unknown({v})"),
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
}

fn cover_tile(path: &std::path::Path, src_crs: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut decoder = Decoder::new(reader)?;

    let (width, height) = decoder.dimensions()?;
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
    let transform = if let Some(matrix) = decoder
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
        // World file C/F are at center of upper-left pixel, so corners are at -0.5/+0.5 offsets.
        -0.5
    } else if raster_type == Some(2) {
        -0.5
    } else {
        0.0
    };

    let corners_px = [
        Pt {
            x: raster_offset,
            y: raster_offset,
        }, // upper-left
        Pt {
            x: width as f64 + raster_offset,
            y: raster_offset,
        }, // upper-right
        Pt {
            x: width as f64 + raster_offset,
            y: height as f64 + raster_offset,
        }, // lower-right
        Pt {
            x: raster_offset,
            y: height as f64 + raster_offset,
        }, // lower-left
    ];

    let corners_src = corners_px.map(|p| transform.apply(p));

    let tf = Proj::new_known_crs(&source_crs, "EPSG:3857")?;
    let corners_merc = corners_src
        .map(|p| tf.transform2((p.x, p.y)).map(|(x, y)| Pt { x, y }))
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;

    let largest_edge = largest_edge_length(&corners_merc)?;
    let z = zoom_for_tile_size(largest_edge);
    let mut tiles = BTreeSet::new();
    for corner in &corners_merc {
        let (x, y) = webmerc_to_tile(corner.x, corner.y, z);
        tiles.insert((x, y));
    }

    println!("File: {}", path.display());
    println!("Source CRS: {source_crs}");
    println!("Transform source: {}", if used_tfw { "world file (.tfw)" } else { "GeoTIFF tags" });
    println!("Largest image edge in EPSG:3857: {largest_edge}");
    println!("Selected zoom: {z}");
    println!("Covering tiles ({}):", tiles.len());
    for (i, (x, y)) in tiles.iter().enumerate() {
        let tile_bounds = tile_bounds_webmerc(z, *x, *y);
        println!("  Tile {i}: z={z}, x={x}, y={y}");
        println!(
            "    corners UL=({}, {}), UR=({}, {}), LR=({}, {}), LL=({}, {})",
            tile_bounds.ul.x,
            tile_bounds.ul.y,
            tile_bounds.ur.x,
            tile_bounds.ur.y,
            tile_bounds.lr.x,
            tile_bounds.lr.y,
            tile_bounds.ll.x,
            tile_bounds.ll.y
        );
    }

    Ok(())
}

fn read_world_file_transform(path: &std::path::Path) -> Result<Option<GeoTransform>, Box<dyn std::error::Error>> {
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

    let d0 = distance(corners[0], corners[1]); // UL-UR
    let d1 = distance(corners[1], corners[2]); // UR-LR
    let d2 = distance(corners[2], corners[3]); // LR-LL
    let d3 = distance(corners[3], corners[0]); // LL-UL

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
