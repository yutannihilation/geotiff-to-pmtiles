use std::collections::BTreeSet;
use std::fs::File;

use pmtiles::{Compression, PmTilesWriter, TileCoord, TileId, TileType};
use proj_lite::Proj;

use crate::cli::Resampling;
use crate::resample::{
    encode_avif, load_raster, load_source_metadata, parse_nodeta, render_tile_debug_multi,
    source_corners_merc_meta, tile_bounds_webmerc, tile_corners_in_source_raster_meta,
    webmerc_to_tile, zoom_for_tile_size,
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
    println!("Input pattern: {input}; loading metadata...");
    let sources = load_source_metadata(input, src_crs)?;
    println!("Loaded metadata for {} source file(s)", sources.len());

    let mut corners_merc = Vec::new();
    let mut source_bounds = Vec::with_capacity(sources.len());
    for source in &sources {
        let corners = source_corners_merc_meta(source)?;
        let min_x = corners.iter().map(|p| p.x).fold(f64::INFINITY, f64::min);
        let max_x = corners
            .iter()
            .map(|p| p.x)
            .fold(f64::NEG_INFINITY, f64::max);
        let min_y = corners.iter().map(|p| p.y).fold(f64::INFINITY, f64::min);
        let max_y = corners
            .iter()
            .map(|p| p.y)
            .fold(f64::NEG_INFINITY, f64::max);
        source_bounds.push((min_x, min_y, max_x, max_y));
        corners_merc.extend_from_slice(&corners);
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

    // Default zoom heuristic: choose the coarsest zoom whose tile edge can still cover
    // the largest side of the union extent. Higher zooms are added as a small pyramid.
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
        // Cover the full extent by taking the inclusive tile range from bbox corners.
        let (x_min, y_min) = webmerc_to_tile(min_x_merc, max_y_merc, z);
        let (x_max, y_max) = webmerc_to_tile(max_x_merc, min_y_merc, z);

        let mut tiles = BTreeSet::new();
        for y in y_min..=y_max {
            for x in x_min..=x_max {
                tiles.insert((x, y));
            }
        }
        let mut tile_list = Vec::with_capacity(tiles.len());
        for (x, y) in tiles {
            let coord = TileCoord::new(z, x, y)?;
            let tile_id = TileId::from(coord).value();
            tile_list.push((tile_id, x, y));
        }
        tile_list.sort_by_key(|(tile_id, _, _)| *tile_id);

        let total_tiles = tile_list.len();
        println!("z={z}: rendering {total_tiles} tile(s) [0%]");

        let mut reported_bucket = 0usize;
        for (idx, (_tile_id, x, y)) in tile_list.into_iter().enumerate() {
            let bounds = tile_bounds_webmerc(z, x, y);
            let tile_merc_corners = [bounds.ul, bounds.ur, bounds.lr, bounds.ll];
            let tile_min_x = bounds.ul.x.min(bounds.lr.x);
            let tile_max_x = bounds.ul.x.max(bounds.lr.x);
            let tile_min_y = bounds.ul.y.min(bounds.lr.y);
            let tile_max_y = bounds.ul.y.max(bounds.lr.y);

            let mut loaded_sources = Vec::new();
            let mut corners_per_source = Vec::new();
            for (source, (smin_x, smin_y, smax_x, smax_y)) in
                sources.iter().zip(source_bounds.iter())
            {
                let intersects = !(tile_max_x < *smin_x
                    || tile_min_x > *smax_x
                    || tile_max_y < *smin_y
                    || tile_min_y > *smax_y);
                if !intersects {
                    continue;
                }
                let corners = tile_corners_in_source_raster_meta(source, tile_merc_corners)?;
                let raster = load_raster(source.path.as_path())?;
                loaded_sources.push(raster);
                corners_per_source.push(corners);
            }

            let per_source = loaded_sources
                .iter()
                .zip(corners_per_source.iter())
                .map(|(raster, corners)| (raster, *corners))
                .collect::<Vec<_>>();

            let rgba = render_tile_debug_multi(&per_source, resampling, nodata);
            let avif = encode_avif(&rgba)?;
            let coord = TileCoord::new(z, x, y)?;
            writer.add_raw_tile(coord, &avif)?;

            let done = idx + 1;
            let percent = (done * 100) / total_tiles.max(1);
            let bucket = percent / 10;
            if bucket > reported_bucket {
                reported_bucket = bucket;
                println!("z={z}: {percent}% ({done}/{total_tiles})");
            }
        }
        println!("z={z}: complete [100%]");
    }

    writer.finalize()?;
    Ok(())
}
