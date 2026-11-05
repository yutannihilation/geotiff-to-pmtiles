mod georef;
mod inputs;
mod nodata;
mod render;
mod types;

pub(crate) const TILE_SIZE: usize = 512;
pub(crate) const DEFAULT_AVIF_QUALITY: u8 = 55;
pub(crate) const DEFAULT_AVIF_SPEED: u8 = 4;
pub(crate) const DEFAULT_WEBP_QUALITY: u8 = 75;
/// Tolerated tile → source reprojection interpolation error, in output pixels.
pub(crate) const MAX_REPROJECTION_ERROR_PX: f64 = 0.125;

pub(crate) use georef::{
    TAG_PLANAR_CONFIGURATION, inverse_projection, read_source_metadata, source_corners_merc_georef,
    tile_mapping_in_georef_raster,
};
pub(crate) use inputs::load_source_metadata;
pub(crate) use nodata::parse_nodata;
pub(crate) use render::{encode_avif, encode_png, encode_webp, make_avif_encoder};
pub(crate) use types::{
    GeoTransform, Georef, NoDataSpec, Pt, SourceMetadata, TileBounds, TileMapping,
    tile_bounds_webmerc, webmerc_to_tile, zoom_for_tile_size,
};
