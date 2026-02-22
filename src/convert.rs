use std::collections::BTreeSet;
use std::fs::File;

use pmtiles::{Compression, PmTilesWriter, TileCoord, TileId, TileType};
use proj_lite::Proj;
use rayon::prelude::*;

use crate::cli::Resampling;
use crate::resample::{
    Pt, encode_avif, largest_edge_length, load_raster, parse_nodeta, read_georef, render_tile_debug,
    tile_bounds_webmerc, webmerc_to_tile, zoom_for_tile_size,
};

pub fn convert(
    input: &std::path::Path,
    output: &std::path::Path,
    src_crs: Option<&str>,
    nodeta: Option<&str>,
    min_zoom_opt: Option<u8>,
    max_zoom_opt: Option<u8>,
    resampling: Resampling,
) -> Result<(), Box<dyn std::error::Error>> {
    let nodata = parse_nodeta(nodeta)?;
    let raster = load_raster(input)?;
    let georef = read_georef(input, src_crs)?;

    let corners_px = [
        Pt {
            x: georef.raster_offset,
            y: georef.raster_offset,
        },
        Pt {
            x: raster.width as f64 + georef.raster_offset,
            y: georef.raster_offset,
        },
        Pt {
            x: raster.width as f64 + georef.raster_offset,
            y: raster.height as f64 + georef.raster_offset,
        },
        Pt {
            x: georef.raster_offset,
            y: raster.height as f64 + georef.raster_offset,
        },
    ];
    let corners_src = corners_px.map(|p| georef.forward.apply(p));

    let to_merc = Proj::new_known_crs(&georef.source_crs, "EPSG:3857")?;
    let corners_merc = corners_src
        .map(|p| to_merc.transform2((p.x, p.y)).map(|(x, y)| Pt { x, y }))
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;

    let auto_min_zoom = zoom_for_tile_size(largest_edge_length(&corners_merc)?);
    let min_zoom = min_zoom_opt.unwrap_or(auto_min_zoom);
    if min_zoom > 31 {
        return Err(format!("min_zoom must be <= 31, got {min_zoom}").into());
    }
    let max_zoom = max_zoom_opt.unwrap_or(min_zoom.saturating_add(3).min(31));
    if max_zoom > 31 {
        return Err(format!("max_zoom must be <= 31, got {max_zoom}").into());
    }
    if max_zoom < min_zoom {
        return Err(format!("max_zoom ({max_zoom}) must be >= min_zoom ({min_zoom})").into());
    }

    let to_wgs84 = Proj::new_known_crs("EPSG:3857", "EPSG:4326")?;
    let mut min_lon = f64::INFINITY;
    let mut min_lat = f64::INFINITY;
    let mut max_lon = f64::NEG_INFINITY;
    let mut max_lat = f64::NEG_INFINITY;
    for p in &corners_merc {
        let (lon, lat) = to_wgs84.transform2((p.x, p.y))?;
        min_lon = min_lon.min(lon);
        min_lat = min_lat.min(lat);
        max_lon = max_lon.max(lon);
        max_lat = max_lat.max(lat);
    }

    let center_lon = (min_lon + max_lon) / 2.0;
    let center_lat = (min_lat + max_lat) / 2.0;

    let file = File::create(output)?;
    let mut writer = PmTilesWriter::new(TileType::Avif)
        .tile_compression(Compression::None)
        .min_zoom(min_zoom)
        .max_zoom(max_zoom)
        .bounds(min_lon, min_lat, max_lon, max_lat)
        .center_zoom(min_zoom)
        .center(center_lon, center_lat)
        .create(file)?;

    let from_merc = Proj::new_known_crs("EPSG:3857", &georef.source_crs)?;
    let inverse = georef.forward.invert()?;

    println!("Input: {}", input.display());
    println!("Output: {}", output.display());
    println!("Source CRS: {}", georef.source_crs);
    println!("Zoom range: {min_zoom}..{max_zoom}");

    let min_x_merc = corners_merc
        .iter()
        .map(|p| p.x)
        .fold(f64::INFINITY, f64::min);
    let max_x_merc = corners_merc
        .iter()
        .map(|p| p.x)
        .fold(f64::NEG_INFINITY, f64::max);
    let min_y_merc = corners_merc
        .iter()
        .map(|p| p.y)
        .fold(f64::INFINITY, f64::min);
    let max_y_merc = corners_merc
        .iter()
        .map(|p| p.y)
        .fold(f64::NEG_INFINITY, f64::max);

    for z in min_zoom..=max_zoom {
        let (x_min, y_min) = webmerc_to_tile(min_x_merc, max_y_merc, z);
        let (x_max, y_max) = webmerc_to_tile(max_x_merc, min_y_merc, z);

        let mut tiles = BTreeSet::new();
        for y in y_min..=y_max {
            for x in x_min..=x_max {
                tiles.insert((x, y));
            }
        }

        let mut work_items = Vec::with_capacity(tiles.len());
        for (x, y) in tiles {
            let bounds = tile_bounds_webmerc(z, x, y);
            let tile_merc_corners = [bounds.ul, bounds.ur, bounds.lr, bounds.ll];

            let mut tile_raster_corners = [Pt { x: 0.0, y: 0.0 }; 4];
            for (i, p) in tile_merc_corners.iter().enumerate() {
                let (sx, sy) = from_merc.transform2((p.x, p.y))?;
                tile_raster_corners[i] = inverse.apply(Pt { x: sx, y: sy });
            }
            work_items.push((z, x, y, tile_raster_corners));
        }

        let mut encoded_tiles = work_items
            .into_par_iter()
            .map(|(z, x, y, corners)| -> Result<(u64, TileCoord, Vec<u8>), String> {
                let rgba = render_tile_debug(&raster, corners, resampling, nodata);
                let avif = encode_avif(&rgba).map_err(|e| e.to_string())?;
                let coord = TileCoord::new(z, x, y).map_err(|e| e.to_string())?;
                let tile_id = TileId::from(coord).value();
                Ok((tile_id, coord, avif))
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;

        encoded_tiles.sort_by_key(|(tile_id, _, _)| *tile_id);
        for (_, coord, avif) in encoded_tiles {
            writer.add_raw_tile(coord, &avif)?;
        }
    }

    writer.finalize()?;
    Ok(())
}
