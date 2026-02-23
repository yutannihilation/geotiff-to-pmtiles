mod georef;
mod inputs;
mod nodata;
mod raster;
mod render;
mod types;

use std::collections::BTreeSet;

use crate::cli::Resampling;

pub(crate) use georef::{
    read_georef, source_corners_merc, source_corners_merc_georef, tile_corners_in_georef_raster,
    tile_corners_in_source_raster,
};
pub(crate) use inputs::{load_source_metadata, load_sources};
pub(crate) use nodata::parse_nodeta;
pub(crate) use raster::{decoding_result_to_u8, load_raster};
pub(crate) use render::{encode_avif, lerp, render_tile_debug_multi, write_avif};
pub(crate) use types::{
    GeoTransform, Georef, NoDataSpec, Pt, Raster, SourceDataset, SourceMetadata,
    tile_bounds_webmerc, webmerc_to_tile, zoom_for_tile_size,
};

pub fn resample_tiles(
    input: &[String],
    src_crs: Option<&str>,
    nodeta: Option<&str>,
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
    // Pick one "base" zoom where a tile edge is at least the largest dataset edge.
    // This intentionally yields 1-4 tiles that cover the union extent.
    let largest_edge = (max_x_merc - min_x_merc).max(max_y_merc - min_y_merc);
    let z = zoom_for_tile_size(largest_edge);

    let (x_min, y_min) = webmerc_to_tile(min_x_merc, max_y_merc, z);
    let (x_max, y_max) = webmerc_to_tile(max_x_merc, min_y_merc, z);
    let mut tiles = BTreeSet::new();
    for y in y_min..=y_max {
        for x in x_min..=x_max {
            tiles.insert((x, y));
        }
    }

    println!("Input args: {}", input.join(" "));
    println!("Input files: {}", sources.len());
    println!("Selected zoom: {z}");
    println!("Output tiles: {}", tiles.len());

    for (idx, (x, y)) in tiles.iter().enumerate() {
        let bounds = tile_bounds_webmerc(z, *x, *y);
        let tile_merc_corners = [bounds.ul, bounds.ur, bounds.lr, bounds.ll];
        let mut per_source = Vec::with_capacity(sources.len());
        for source in &sources {
            let corners = tile_corners_in_source_raster(source, tile_merc_corners)?;
            per_source.push((&source.raster, corners));
        }
        let out = render_tile_debug_multi(&per_source, resampling, nodata);
        let filename = format!("out{}.avif", idx + 1);
        write_avif(&filename, &out)?;

        println!("  wrote {} for z={}, x={}, y={}", filename, z, x, y);
    }

    Ok(())
}
