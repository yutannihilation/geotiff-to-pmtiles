# geotiff-to-pmtiles

> [!WARNING]
> This project is under active development. Use with caution!

A simple CLI for converting GeoTIFF files to PMTiles.

```sh
geotiff-to-pmtiles /path/to/*.tif
```

Compared to the existing solutions:

- Single statically linked binary with no external runtime dependencies.
- Supports multiple input arguments, so no pre-merge step with `gdal merge` is needed.
- Outputs AVIF tiles.

## Installation

Pre-built binaries can be found at [Releases](https://github.com/yutannihilation/geotiff-to-pmtiles/releases).

## Usages

```
Usage: geotiff-to-pmtiles [OPTIONS] <INPUT>...

Arguments:
  <INPUT>...  Input GeoTIFF path(s) and/or glob pattern(s) (e.g. data/*.tif data/a.tif)

Options:
  -o, --output <OUTPUT>          Output PMTiles path [default: out.pmtiles]
      --src-crs <SRC_CRS>        Source CRS when GeoKeyDirectoryTag is missing (e.g. "EPSG:4326")
      --nodata <NODATA>          NoData value, e.g. "0" or "255,255,255"
      --min-zoom <MIN_ZOOM>      Minimum zoom level. If omitted, it is auto-determined
      --max-zoom <MAX_ZOOM>      Maximum zoom level. If omitted, defaults to min_zoom + 3
      --resampling <RESAMPLING>  Resampling method [default: bilinear] [possible values: nearest, bilinear]
      --cache-mb <CACHE_MB>      Global chunk cache size in MiB for TIFF partial reads [default: 128]
      --quality <AVIF_QUALITY>   AVIF quality in the range 1..=100 (higher is better quality, larger files) [default: 55]
      --speed <AVIF_SPEED>       AVIF speed in the range 1..=10 (lower is slower but better compression) [default: 4]
  -h, --help                     Print help
```

### Examples

```sh
# specify output (default: out.pmtiles)
geotiff-to-pmtiles -o /path/to/out.pmtiles /path/to/*.tif

# specify zoom levels (defaults: min zoom auto, max zoom = min + 3)
geotiff-to-pmtiles --min-zoom 14 --max-zoom 18 /path/to/*.tif

# if CRS is missing, use --src-crs option
geotiff-to-pmtiles --src-crs EPSG:6677 /path/to/*.tif
```

## Notes

- Accepts one or more input arguments.
- Each input argument can be either a file path or a glob pattern (for example `/path/to/*.tif`).
- Output path is specified with `--output` (`-o`) and defaults to `out.pmtiles`.
- To force in-app glob expansion consistently across shells, quote glob patterns.
- If GeoTIFF georeferencing tags are missing, the tool falls back to adjacent world files (`.tfw`, `.TFW`, `.tifw`, `.TIFW`) when available.
- `--src-crs` is required when CRS metadata is missing.
- `--nodata` supports values like `0` or `255,255,255` and maps nodata output to alpha `0`.
- Resampling methods:
  - `nearest`: chooses nearest valid sample.
  - `bilinear`: weighted interpolation that ignores invalid/nodata neighbors.

## Development Utilities

Generate benchmark GeoTIFF/world-file fixtures with:

```sh
cargo run --manifest-path tools/generate-bench-data/Cargo.toml
```
