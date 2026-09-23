use proj_lite::Proj;
use tiff::decoder::Decoder;
use tiff::tags::Tag;

use super::{GeoTransform, Georef, Pt, TILE_SIZE, TileBounds, TileMapping};

// Well-known TIFF/GeoTIFF tag IDs not defined in the `tiff` crate.
pub(crate) const TAG_PLANAR_CONFIGURATION: Tag = Tag::Unknown(284);
const TAG_MODEL_PIXEL_SCALE: Tag = Tag::Unknown(33550);
const TAG_MODEL_TIEPOINT: Tag = Tag::Unknown(33922);
const TAG_MODEL_TRANSFORMATION: Tag = Tag::Unknown(34264);
const TAG_GEO_KEY_DIRECTORY: Tag = Tag::Unknown(34735);
const TAG_GDAL_NODATA: Tag = Tag::Unknown(42113);

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

/// Web Mercator → source CRS transform used by [`tile_mapping_in_georef_raster`].
pub(crate) fn inverse_projection(georef: &Georef) -> Result<Proj, Box<dyn std::error::Error>> {
    Ok(Proj::new_known_crs("EPSG:3857", &georef.source_crs)?)
}

/// Build the tile → source raster mapping for one output tile.
///
/// The tile is an axis-aligned rectangle in Web Mercator, so a UV position is
/// first placed linearly inside `bounds`, then reprojected into the source CRS
/// and mapped to raster pixels with the inverse georeferencing transform.
/// [`TileMapping::refine`] evaluates this exact mapping at as many control
/// points as needed to keep the interpolation error within `max_error_out_px`.
///
/// `from_merc` must be the `EPSG:3857` → `georef.source_crs` transform (see
/// [`inverse_projection`]); it is passed in so callers can reuse it across tiles.
pub(crate) fn tile_mapping_in_georef_raster(
    georef: &Georef,
    from_merc: &Proj,
    bounds: &TileBounds,
    max_error_out_px: f64,
) -> Result<TileMapping, Box<dyn std::error::Error>> {
    let inverse = georef.forward.invert()?;
    let ul = bounds.ul;
    let width = bounds.ur.x - bounds.ul.x;
    let height = bounds.ll.y - bounds.ul.y;
    TileMapping::refine(max_error_out_px, TILE_SIZE, |u, v| {
        let merc_x = ul.x + width * u;
        let merc_y = ul.y + height * v;
        let (sx, sy) = from_merc.transform2((merc_x, merc_y))?;
        Ok(inverse.apply(Pt { x: sx, y: sy }))
    })
}

/// Open a TIFF once and extract dimensions, georeferencing, and GDAL_NODATA.
pub(crate) fn read_source_metadata(
    path: std::path::PathBuf,
    src_crs: Option<&str>,
) -> Result<super::SourceMetadata, Box<dyn std::error::Error>> {
    let file = std::fs::File::open(&path)?;
    let mut decoder = Decoder::new(std::io::BufReader::new(file))?;

    let (w, h) = decoder.dimensions()?;

    let gdal_nodata = decoder.get_tag_ascii_string(TAG_GDAL_NODATA).ok();

    let georef = read_georef_from_decoder(&mut decoder, src_crs, &path)?;

    Ok(super::SourceMetadata {
        path,
        width: w as usize,
        height: h as usize,
        georef,
        gdal_nodata,
    })
}

fn read_georef_from_decoder<R: std::io::Read + std::io::Seek>(
    decoder: &mut Decoder<R>,
    src_crs: Option<&str>,
    path: &std::path::Path,
) -> Result<Georef, Box<dyn std::error::Error>> {
    let geokey_directory: Option<Vec<u16>> = decoder
        .get_tag_u32_vec(TAG_GEO_KEY_DIRECTORY)
        .ok()
        .map(|v| v.iter().map(|&x| x as u16).collect());

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

    let model_transformation: Option<Vec<f64>> =
        decoder.get_tag_f64_vec(TAG_MODEL_TRANSFORMATION).ok();
    let pixel_scale: Option<Vec<f64>> = decoder.get_tag_f64_vec(TAG_MODEL_PIXEL_SCALE).ok();
    let tie_points: Option<Vec<f64>> = decoder.get_tag_f64_vec(TAG_MODEL_TIEPOINT).ok();

    let forward = if let Some(matrix) = model_transformation {
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
    } else if let (Some(pixel_scale), Some(tie_points)) = (pixel_scale, tie_points) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resample::{tile_bounds_webmerc, webmerc_to_tile};

    fn wgs84_georef() -> Georef {
        Georef {
            source_crs: "EPSG:4326".to_string(),
            forward: GeoTransform::TiePointAndPixelScale {
                raster_x: 0.0,
                raster_y: 0.0,
                model_x: 20.0,
                model_y: 62.0,
                scale_x: 0.0005,
                scale_y: 0.0005,
            },
            raster_offset: 0.0,
        }
    }

    /// Max interpolation error of `mapping` in output pixels over a dense sample.
    fn dense_error_out_px(georef: &Georef, bounds: &TileBounds, mapping: &TileMapping) -> f64 {
        let from_merc = inverse_projection(georef).unwrap();
        let inverse = georef.forward.invert().unwrap();
        let ul = mapping.point(0, 0);
        let ur = mapping.point(mapping.cells(), 0);
        let ll = mapping.point(0, mapping.cells());
        let scale = (ur.x - ul.x)
            .hypot(ur.y - ul.y)
            .min((ll.x - ul.x).hypot(ll.y - ul.y))
            / TILE_SIZE as f64;
        let mut worst = 0.0_f64;
        for j in 0..=128 {
            for i in 0..=128 {
                let (u, v) = (i as f64 / 128.0, j as f64 / 128.0);
                let mx = bounds.ul.x + (bounds.ur.x - bounds.ul.x) * u;
                let my = bounds.ul.y + (bounds.ll.y - bounds.ul.y) * v;
                let (sx, sy) = from_merc.transform2((mx, my)).unwrap();
                let exact = inverse.apply(Pt { x: sx, y: sy });
                let approx = mapping.eval(u, v);
                worst = worst.max((exact.x - approx.x).hypot(exact.y - approx.y) / scale);
            }
        }
        worst
    }

    fn helsinki_tile(z: u8) -> TileBounds {
        let to_merc = Proj::new_known_crs("EPSG:4326", "EPSG:3857").unwrap();
        let (mx, my) = to_merc.transform2((24.9, 60.2)).unwrap();
        let (x, y) = webmerc_to_tile(mx, my, z);
        tile_bounds_webmerc(z, x, y)
    }

    #[test]
    fn coarse_tile_is_refined_and_meets_error_target() {
        let georef = wgs84_georef();
        let bounds = helsinki_tile(4);
        let from_merc = inverse_projection(&georef).unwrap();
        let mapping = tile_mapping_in_georef_raster(&georef, &from_merc, &bounds, 0.125).unwrap();
        // Corner-only mapping is off by ~22 output px here; 16 cells bring it under 0.125.
        assert!(mapping.cells() >= 16, "cells = {}", mapping.cells());
        let err = dense_error_out_px(&georef, &bounds, &mapping);
        assert!(err <= 0.125, "error {err} output px");
    }

    #[test]
    fn fine_tile_keeps_corner_only_mapping() {
        let georef = wgs84_georef();
        let bounds = helsinki_tile(16);
        let from_merc = inverse_projection(&georef).unwrap();
        let mapping = tile_mapping_in_georef_raster(&georef, &from_merc, &bounds, 0.125).unwrap();
        assert_eq!(mapping.cells(), 1);
        let err = dense_error_out_px(&georef, &bounds, &mapping);
        assert!(err <= 0.125, "error {err} output px");
    }
}
