# geotiff-to-pmtiles

A simple CLI for converting GeoTIFF files to PMTiles.

```sh
geotiff-to-pmtiles /path/to/*.tif
```

Compared to the existing solutions:

- Single statically linked binary with no external runtime dependencies.
- Supports multiple input TIFF files directly (i.e. so no pre-merge step with `gdal merge` or `gdalbuildvrt`).
- Supports AVIF, PNG, and WebP tiles.

## Installation

Pre-built binaries can be found at [Releases](https://github.com/yutannihilation/geotiff-to-pmtiles/releases).

## Usages

```
Usage: geotiff-to-pmtiles [OPTIONS] <INPUT>...

Arguments:
  <INPUT>...  Input GeoTIFF path(s) and/or glob pattern(s) (e.g. data/*.tif data/a.tif)

Options:
  -o, --output <OUTPUT>          Output PMTiles path [default: out.pmtiles]
      --serve                    Serve requested tiles dynamically instead of creating a PMTiles file
      --bind <BIND>              Address used by preview server mode [default: 127.0.0.1:3000]
      --cache-mb <CACHE_MB>      Decoded TIFF chunk cache size in MiB for preview server mode [default: 128]
      --src-crs <SRC_CRS>        Source CRS when GeoKeyDirectoryTag is missing (e.g. "EPSG:4326")
      --nodata <NODATA>          NoData value, e.g. "0" or "255,255,255"
      --min-zoom <MIN_ZOOM>      Minimum zoom level. If omitted, it is auto-determined
      --max-zoom <MAX_ZOOM>      Maximum zoom level. If omitted, defaults to min_zoom + 3
      --resampling <RESAMPLING>  Resampling method [default: bilinear] [possible values: nearest, bilinear]
      --tile-format <TILE_FORMAT>  Tile image format [default: avif] [possible values: avif, png, webp-lossless, webp-lossy]
      --quality <AVIF_QUALITY>   AVIF quality in the range 1..=100 (higher is better quality, larger files) [default: 55]
      --speed <AVIF_SPEED>       AVIF speed in the range 1..=10 (lower is slower but better compression) [default: 4]
      --png-compression <PNG_COMPRESSION>  PNG compression preset [default: default] [possible values: fast, default, best]
      --webp-quality <WEBP_QUALITY>  WebP quality for lossy mode (1..=100) [default: 75]
  -h, --help                     Print help
```

### Examples

```sh
# specify output (default: out.pmtiles)
geotiff-to-pmtiles -o /path/to/out.pmtiles /path/to/*.tif

# specify zoom levels (defaults: min zoom auto, max zoom = min + 3)
geotiff-to-pmtiles --min-zoom 14 --max-zoom 18 /path/to/*.tif

# photo tiles (e.g. ortho): lossy AVIF + bilinear (default)
geotiff-to-pmtiles /path/to/*.tif

# data tiles (e.g. DEM, land cover): lossless PNG + nearest
geotiff-to-pmtiles --tile-format png --resampling nearest /path/to/*.tif

# if CRS is missing, use --src-crs option
geotiff-to-pmtiles --src-crs EPSG:6677 /path/to/*.tif

# preview tiles without creating a PMTiles file
geotiff-to-pmtiles --serve --tile-format png /path/to/*.tif
# then use http://127.0.0.1:3000/tiles/{z}/{x}/{y}.png as an XYZ source
```

## Preview server

`--serve` loads the fixed input GeoTIFF metadata once and renders only requested
XYZ tiles. Decoded TIFF chunks are retained in a byte-bounded LRU cache controlled
by `--cache-mb`. The server returns `204 No Content` for tiles outside the source
coverage or tiles that render fully transparent. Press Ctrl+C to stop it.

## Output tile format

All tiles are 512×512 pixels. When using the tiles in a map viewer, set `tileSize: 512` (e.g. in MapLibre GL JS or Leaflet).

| Format | Encoding | File size | Speed | Best for |
|--------|----------|-----------|-------|----------|
| AVIF (default) | Lossy | Smallest | Slow | Photo imagery (ortho, satellite) |
| WebP lossy | Lossy | Small | Medium | Photo imagery, wider viewer support |
| WebP lossless | Lossless | Medium | Medium | Data rasters with good compression |
| PNG | Lossless | Largest | Fast | Data rasters (DEM, land cover) |

**Important:** Lossy encoding (AVIF, WebP lossy) alters pixel values, which corrupts data tiles like DEM where exact values matter. For data rasters, use a lossless format (`png` or `webp-lossless`) with `--resampling nearest` to preserve original values.

## Notes
- If GeoTIFF georeferencing tags are missing, the tool falls back to adjacent world files (`.tfw`, `.TFW`, `.tifw`, `.TIFW`) when available.
- `--src-crs` is required when CRS metadata is missing.
- `--nodata` supports values like `0` or `255,255,255` and maps nodata output to alpha `0`.
- Resampling methods:
  - `nearest`: chooses nearest valid sample.
  - `bilinear`: weighted interpolation that ignores invalid/nodata neighbors.

## Unsupported TIFF Features

This tool targets the most common GeoTIFF configurations. The following rare
features are intentionally unsupported to keep the codebase simple:

- **Planar configuration** — Only chunky (interleaved) pixel layout
  (`PlanarConfiguration=1`) is supported. Separate-plane TIFFs
  (`PlanarConfiguration=2`) are rejected at load time. Most GeoTIFF writers
  default to chunky.
- **JPEG-in-TIFF compression** — TIFF compression 7 (JPEG) is not supported.
  JPEG-compressed GeoTIFFs are uncommon; Deflate and LZW are the standard
  choices for lossless GeoTIFF distribution.

If you encounter one of these, convert the file beforehand with GDAL:

```sh
# Re-encode to Deflate, chunky layout
gdal raster convert input.tif output.tif --co COMPRESS=DEFLATE --co INTERLEAVE=PIXEL
```

## Development Utilities

Generate benchmark GeoTIFF/world-file fixtures with:

```sh
cargo run --manifest-path tools/generate-bench-data/Cargo.toml
```
