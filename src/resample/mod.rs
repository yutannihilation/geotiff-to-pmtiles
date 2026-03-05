mod georef;
mod inputs;
mod nodata;
mod render;
mod types;

pub(crate) const TILE_SIZE: usize = 256;
pub(crate) const DEFAULT_AVIF_QUALITY: u8 = 55;
pub(crate) const DEFAULT_AVIF_SPEED: u8 = 4;

pub(crate) use georef::{read_georef, source_corners_merc_georef, tile_corners_in_georef_raster};
pub(crate) use inputs::load_source_metadata;
pub(crate) use nodata::parse_nodata;
pub(crate) use render::{encode_avif, lerp};
pub(crate) use types::{
    GeoTransform, Georef, NoDataSpec, Pt, SourceMetadata, tile_bounds_webmerc, webmerc_to_tile,
    zoom_for_tile_size,
};
