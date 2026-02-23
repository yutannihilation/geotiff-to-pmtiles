# geotiff-to-pmtiles

> [!WARNING]
> This project is under active development. Use with caution!

A simple CLI for converting GeoTIFF files to PMTiles.

```sh
geotiff-to-pmtiles convert /path/to/*.tif
```

Compared to the existing solutions:

- Single statically linked binary with no external runtime dependencies.
- Supports multiple input arguments, so no pre-merge step with `gdal merge` is needed.
- Memory efficient by design (with the tradeoff of more repeated disk reads).
- Outputs AVIF tiles.

## Installation

Pre-built binaries can be found at [Releases](https://github.com/yutannihilation/geotiff-to-pmtiles/releases).

## Usages

### Convert to PMTiles

```sh
# defaults: min zoom auto, max zoom = min + 3
geotiff-to-pmtiles convert -o out.pmtiles /path/to/*.tif

# if CRS is missing, use --src-crs option
geotiff-to-pmtiles convert --src-crs EPSG:6677 -o out.pmtiles /path/to/*.tif
```

### Debug commands

```sh
# Header dump
geotiff-to-pmtiles dump-header /path/to/sample.tif

# Find 1-4 covering tiles at an auto-selected zoom
geotiff-to-pmtiles cover-tile /path/to/sample.tif --src-crs EPSG:6677

# Debug render covering tiles as out1.avif, out2.avif, ...
geotiff-to-pmtiles resample-tiles /path/to/*.tif --src-crs EPSG:6677 --resampling bilinear
```

## Notes

- `convert` and `resample-tiles` accept one or more input arguments.
- Each input argument can be either a file path or a glob pattern (for example `/path/to/*.tif`).
- `convert` output path is specified with `--output` (`-o`) and defaults to `out.pmtiles`.
- To force in-app glob expansion consistently across shells, quote glob patterns.
- If GeoTIFF georeferencing tags are missing, the tool falls back to adjacent world files (`.tfw`, `.TFW`, `.tifw`, `.TIFW`) when available.
- `--src-crs` is required when CRS metadata is missing.
- `--nodeta` (alias `--nodata`) supports values like `0` or `255,255,255` and maps nodata output to alpha `0`.
- Resampling methods:
  - `nearest`: chooses nearest valid sample.
  - `bilinear`: weighted interpolation that ignores invalid/nodata neighbors.
