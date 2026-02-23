use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "geotiff-to-pmtiles")]
#[command(about = "Utilities for working with GeoTIFF and PMTiles")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Print TIFF/GeoTIFF header information.
    DumpHeader {
        /// Path to a GeoTIFF file.
        input: std::path::PathBuf,
    },
    /// Compute the minimum covering Web Mercator Z/X/Y tile for a GeoTIFF.
    CoverTile {
        /// Path to a GeoTIFF file.
        input: std::path::PathBuf,
        /// Source CRS when GeoKeyDirectoryTag is missing (e.g. "EPSG:4326").
        #[arg(long)]
        src_crs: Option<String>,
    },
    /// Resample each covering tile to a 512x512 AVIF image for debugging.
    ResampleTiles {
        /// Input GeoTIFF path(s) and/or glob pattern(s) (e.g. data/*.tif data/a.tif).
        #[arg(required = true, num_args = 1..)]
        input: Vec<String>,
        /// Source CRS when GeoKeyDirectoryTag is missing (e.g. "EPSG:4326").
        #[arg(long)]
        src_crs: Option<String>,
        /// NoData value, e.g. "0" or "255,255,255".
        #[arg(long, alias = "nodata")]
        nodeta: Option<String>,
        /// Resampling method.
        #[arg(long, value_enum, default_value_t = Resampling::Bilinear)]
        resampling: Resampling,
    },
    /// Convert GeoTIFF to PMTiles with AVIF image tiles.
    Convert {
        /// Input GeoTIFF path(s) and/or glob pattern(s) (e.g. data/*.tif data/a.tif).
        #[arg(required = true, num_args = 1..)]
        input: Vec<String>,
        /// Output PMTiles path (positional; default: out.pmtiles).
        #[arg(default_value = "out.pmtiles")]
        output: std::path::PathBuf,
        /// Source CRS when GeoKeyDirectoryTag is missing (e.g. "EPSG:4326").
        #[arg(long)]
        src_crs: Option<String>,
        /// NoData value, e.g. "0" or "255,255,255".
        #[arg(long, alias = "nodata")]
        nodeta: Option<String>,
        /// Minimum zoom level. If omitted, it is auto-determined.
        #[arg(long)]
        min_zoom: Option<u8>,
        /// Maximum zoom level. If omitted, defaults to min_zoom + 3.
        #[arg(long)]
        max_zoom: Option<u8>,
        /// Resampling method.
        #[arg(long, value_enum, default_value_t = Resampling::Bilinear)]
        resampling: Resampling,
        /// Global chunk cache size in MiB for TIFF partial reads.
        #[arg(long, default_value_t = 128)]
        cache_mb: usize,
    },
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum Resampling {
    Nearest,
    Bilinear,
}
