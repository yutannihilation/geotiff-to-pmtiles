use std::collections::BTreeSet;
use std::fs::File;

use pmtiles::{Compression, PmTilesWriter, TileCoord, TileId, TileType};
use proj_lite::Proj;
use rayon::prelude::*;

use crate::cli::Resampling;
use crate::resample::{
    encode_avif, load_sources, parse_nodeta, render_tile_debug_multi, source_corners_merc,
    tile_bounds_webmerc, tile_corners_in_source_raster, webmerc_to_tile, zoom_for_tile_size,
};

pub fn convert(
    input: &str,
    output: &std::path::Path,
    src_crs: Option<&str>,
    nodeta: Option<&str>,
    min_zoom_opt: Option<u8>,
    max_zoom_opt: Option<u8>,
    resampling: Resampling,
) -> Result<(), Box<dyn std::error::Error>> {
    let nodata = parse_nodeta(nodeta)?;
    let sources = load_sources(input, src_crs)?;

    let mut corners_merc = Vec::new();
    for source in &sources {
        corners_merc.extend_from_slice(&source_corners_merc(source)?);
    }

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

    let auto_min_zoom = zoom_for_tile_size((max_x_merc - min_x_merc).max(max_y_merc - min_y_merc));
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

    println!("Input pattern: {input}");
    println!("Input files: {}", sources.len());
    println!("Output: {}", output.display());
    println!("Zoom range: {min_zoom}..{max_zoom}");

    for z in min_zoom..=max_zoom {
        let (x_min, y_min) = webmerc_to_tile(min_x_merc, max_y_merc, z);
        let (x_max, y_max) = webmerc_to_tile(max_x_merc, min_y_merc, z);

        let mut tiles = BTreeSet::new();
        for y in y_min..=y_max {
            for x in x_min..=x_max {
                tiles.insert((x, y));
            }
        }

        let mut encoded_tiles = tiles
            .into_par_iter()
            .map(|(x, y)| -> Result<(u64, TileCoord, Vec<u8>), String> {
                let bounds = tile_bounds_webmerc(z, x, y);
                let tile_merc_corners = [bounds.ul, bounds.ur, bounds.lr, bounds.ll];

                let mut per_source = Vec::with_capacity(sources.len());
                for source in &sources {
                    let corners = tile_corners_in_source_raster(source, tile_merc_corners)
                        .map_err(|e| e.to_string())?;
                    per_source.push((&source.raster, corners));
                }

                let rgba = render_tile_debug_multi(&per_source, resampling, nodata);
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
