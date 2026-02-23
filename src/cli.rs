use clap::Parser;

#[derive(Debug, Parser)]
#[command(name = "geotiff-to-pmtiles")]
#[command(about = "Convert GeoTIFF to PMTiles with AVIF image tiles")]
pub struct Cli {
    /// Input GeoTIFF path(s) and/or glob pattern(s) (e.g. data/*.tif data/a.tif).
    #[arg(required = true, num_args = 1..)]
    pub input: Vec<String>,
    /// Output PMTiles path.
    #[arg(short, long, default_value = "out.pmtiles")]
    pub output: std::path::PathBuf,
    /// Source CRS when GeoKeyDirectoryTag is missing (e.g. "EPSG:4326").
    #[arg(long)]
    pub src_crs: Option<String>,
    /// NoData value, e.g. "0" or "255,255,255".
    #[arg(long, alias = "nodata")]
    pub nodeta: Option<String>,
    /// Minimum zoom level. If omitted, it is auto-determined.
    #[arg(long)]
    pub min_zoom: Option<u8>,
    /// Maximum zoom level. If omitted, defaults to min_zoom + 3.
    #[arg(long)]
    pub max_zoom: Option<u8>,
    /// Resampling method.
    #[arg(long, value_enum, default_value_t = Resampling::Bilinear)]
    pub resampling: Resampling,
    /// Global chunk cache size in MiB for TIFF partial reads.
    #[arg(long, default_value_t = 128)]
    pub cache_mb: usize,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum Resampling {
    Nearest,
    Bilinear,
}
