use clap::Parser;
use clap::value_parser;

#[derive(Debug, Parser)]
#[command(name = "geotiff-to-pmtiles")]
#[command(about = "Convert GeoTIFF to PMTiles or serve AVIF/PNG preview tiles")]
pub struct Cli {
    /// Input GeoTIFF path(s) and/or glob pattern(s) (e.g. data/*.tif data/a.tif).
    #[arg(required = true, num_args = 1..)]
    pub input: Vec<String>,
    /// Output PMTiles path.
    #[arg(short, long, default_value = "out.pmtiles")]
    pub output: std::path::PathBuf,
    /// Serve requested tiles dynamically instead of creating a PMTiles file.
    #[arg(long)]
    pub serve: bool,
    /// Address used by preview server mode.
    #[arg(long, default_value = "127.0.0.1:3000")]
    pub bind: String,
    /// Decoded TIFF chunk cache size in MiB for preview server mode.
    #[arg(long, default_value_t = 128)]
    pub cache_mb: usize,
    /// Source CRS when GeoKeyDirectoryTag is missing (e.g. "EPSG:4326").
    #[arg(long)]
    pub src_crs: Option<String>,
    /// NoData value, e.g. "0" or "255,255,255".
    #[arg(long)]
    pub nodata: Option<String>,
    /// Minimum zoom level. If omitted, it is auto-determined.
    #[arg(long)]
    pub min_zoom: Option<u8>,
    /// Maximum zoom level. If omitted, defaults to min_zoom + 3.
    #[arg(long)]
    pub max_zoom: Option<u8>,
    /// Resampling method.
    #[arg(long, value_enum, default_value_t = Resampling::Bilinear)]
    pub resampling: Resampling,
    /// Tile image format.
    #[arg(long = "tile-format", value_enum, default_value_t = TileFormat::Avif)]
    pub tile_format: TileFormat,
    /// AVIF quality in the range 1..=100 (higher is better quality, larger files). Ignored for PNG.
    #[arg(
        long = "quality",
        default_value_t = crate::resample::DEFAULT_AVIF_QUALITY,
        value_parser = value_parser!(u8).range(1..=100)
    )]
    pub avif_quality: u8,
    /// AVIF speed in the range 1..=10 (lower is slower but better compression). Ignored for PNG.
    #[arg(
        long = "speed",
        default_value_t = crate::resample::DEFAULT_AVIF_SPEED,
        value_parser = value_parser!(u8).range(1..=10)
    )]
    pub avif_speed: u8,
    /// PNG compression preset. Ignored for AVIF.
    #[arg(long = "png-compression", value_enum, default_value_t = PngCompression::Balanced)]
    pub png_compression: PngCompression,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum TileFormat {
    Avif,
    Png,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum PngCompression {
    Fast,
    Balanced,
    High,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum Resampling {
    Nearest,
    Bilinear,
}
