use std::collections::BTreeSet;
use std::fs::File;

use pmtiles::{Compression, PmTilesWriter, TileCoord, TileType};
use proj_lite::Proj;

use crate::cli::Resampling;
use crate::resample::{
    Pt, encode_avif, largest_edge_length, load_raster, read_georef, render_tile_debug,
    tile_bounds_webmerc, webmerc_to_tile, zoom_for_tile_size,
};

pub fn convert(
    input: &std::path::Path,
    output: &std::path::Path,
    src_crs: Option<&str>,
    resampling: Resampling,
) -> Result<(), Box<dyn std::error::Error>> {
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

    let min_zoom = zoom_for_tile_size(largest_edge_length(&corners_merc)?);
    let max_zoom = (min_zoom.saturating_add(2)).min(31);

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

    let min_x_merc = corners_merc.iter().map(|p| p.x).fold(f64::INFINITY, f64::min);
    let max_x_merc = corners_merc
        .iter()
        .map(|p| p.x)
        .fold(f64::NEG_INFINITY, f64::max);
    let min_y_merc = corners_merc.iter().map(|p| p.y).fold(f64::INFINITY, f64::min);
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

        for (x, y) in tiles {
            let bounds = tile_bounds_webmerc(z, x, y);
            let tile_merc_corners = [bounds.ul, bounds.ur, bounds.lr, bounds.ll];

            let mut tile_raster_corners = [Pt { x: 0.0, y: 0.0 }; 4];
            for (i, p) in tile_merc_corners.iter().enumerate() {
                let (sx, sy) = from_merc.transform2((p.x, p.y))?;
                tile_raster_corners[i] = inverse.apply(Pt { x: sx, y: sy });
            }

            let rgba = render_tile_debug(&raster, tile_raster_corners, resampling);
            let avif = encode_avif(&rgba)?;
            let coord = TileCoord::new(z, x, y)?;
            writer.add_raw_tile(coord, &avif)?;
        }
    }

    writer.finalize()?;
    Ok(())
}
