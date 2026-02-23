use std::fs::File;
use std::io::BufReader;

use proj_lite::Proj;
use tiff::decoder::Decoder;
use tiff::tags::Tag;

use super::{GeoTransform, Georef, Pt, SourceDataset};

pub(crate) fn source_corners_merc(
    source: &SourceDataset,
) -> Result<[Pt; 4], Box<dyn std::error::Error>> {
    source_corners_merc_from_dims(&source.georef, source.raster.width, source.raster.height)
}

pub(crate) fn source_corners_merc_georef(
    georef: &Georef,
    width: usize,
    height: usize,
) -> Result<[Pt; 4], Box<dyn std::error::Error>> {
    source_corners_merc_from_dims(georef, width, height)
}

fn source_corners_merc_from_dims(
    georef: &Georef,
    width: usize,
    height: usize,
) -> Result<[Pt; 4], Box<dyn std::error::Error>> {
    // Corner coordinates in raster pixel space, adjusted for pixel-is-point/pixel-is-area offset.
    let corners_px = [
        Pt {
            x: georef.raster_offset,
            y: georef.raster_offset,
        },
        Pt {
            x: width as f64 + georef.raster_offset,
            y: georef.raster_offset,
        },
        Pt {
            x: width as f64 + georef.raster_offset,
            y: height as f64 + georef.raster_offset,
        },
        Pt {
            x: georef.raster_offset,
            y: height as f64 + georef.raster_offset,
        },
    ];
    let corners_src = corners_px.map(|p| georef.forward.apply(p));
    // Reproject source CRS -> Web Mercator so tile covering math is done in one common space.
    let to_merc = Proj::new_known_crs(&georef.source_crs, "EPSG:3857")?;
    let mut out = [Pt { x: 0.0, y: 0.0 }; 4];
    for (i, p) in corners_src.iter().enumerate() {
        let (x, y) = to_merc.transform2((p.x, p.y))?;
        out[i] = Pt { x, y };
    }
    Ok(out)
}

pub(crate) fn tile_corners_in_source_raster(
    source: &SourceDataset,
    tile_corners_merc: [Pt; 4],
) -> Result<[Pt; 4], Box<dyn std::error::Error>> {
    tile_corners_in_georef_raster(&source.georef, tile_corners_merc)
}

pub(crate) fn tile_corners_in_georef_raster(
    georef: &Georef,
    tile_corners_merc: [Pt; 4],
) -> Result<[Pt; 4], Box<dyn std::error::Error>> {
    // Reproject tile corners from WebMercator to source CRS, then map CRS -> raster
    // with the inverse georeferencing transform. Rendering interpolates inside these corners.
    let from_merc = Proj::new_known_crs("EPSG:3857", &georef.source_crs)?;
    let inverse = georef.forward.invert()?;
    let mut out = [Pt { x: 0.0, y: 0.0 }; 4];
    for (i, p) in tile_corners_merc.iter().enumerate() {
        let (sx, sy) = from_merc.transform2((p.x, p.y))?;
        out[i] = inverse.apply(Pt { x: sx, y: sy });
    }
    Ok(out)
}

pub(crate) fn read_georef(
    path: &std::path::Path,
    src_crs: Option<&str>,
) -> Result<Georef, Box<dyn std::error::Error>> {
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
    // Prefer explicit GeoTIFF transform tags, then fallback to adjacent world file.
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

    // RasterType=PixelIsPoint (or world-file semantics) is modeled via half-pixel shift.
    let raster_offset = if used_tfw || raster_type == Some(2) {
        -0.5
    } else {
        0.0
    };

    Ok(Georef {
        source_crs,
        forward,
        raster_offset,
    })
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
