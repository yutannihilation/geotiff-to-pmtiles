# geotiff-to-pmtiles

Small Rust CLI to inspect GeoTIFF headers, estimate covering XYZ tiles, debug resampling, and convert GeoTIFF inputs into PMTiles with AVIF tiles.

## Installation

Pre-built binaries can be found at [Releases](https://github.com/yutannihilation/geotiff-to-pmtiles/releases).

## Usages

### Convert to PMTiles

```sh
# defaults: min zoom auto, max zoom = min + 3
geotiff-to-pmtiles convert "./data/*.tif" --src-crs EPSG:6677 --output out.pmtiles
```

### Debug commands

```sh
# Header dump
geotiff-to-pmtiles dump-header ./data/sample.tif

# Find 1-4 covering tiles at an auto-selected zoom
geotiff-to-pmtiles cover-tile ./data/sample.tif --src-crs EPSG:6677

# Debug render covering tiles as out1.avif, out2.avif, ...
geotiff-to-pmtiles resample-tiles "./data/*.tif" --src-crs EPSG:6677 --resampling bilinear
```

## Notes

- Input can be a single file or a glob pattern (for example `/path/data/*.tif`).
- If GeoTIFF georeferencing tags are missing, the tool falls back to adjacent world files (`.tfw`, `.TFW`, `.tifw`, `.TIFW`) when available.
- `--src-crs` is required when CRS metadata is missing.
- `--nodeta` (alias `--nodata`) supports values like `0` or `255,255,255` and maps nodata output to alpha `0`.
- Resampling methods:
  - `nearest`: chooses nearest valid sample.
  - `bilinear`: weighted interpolation that ignores invalid/nodata neighbors.
