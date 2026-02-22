# Algorithms and Data Flow

This document describes the current conversion/resampling strategy used by `geotiff-to-pmtiles`.

## Pipeline

1. Expand input path(s): a single file or a glob pattern.
2. Load each raster and georeference transform.
3. Reproject source corners into Web Mercator (`EPSG:3857`).
4. Build tile coverage from union extent.
5. For each tile, map tile corners back into each source raster.
6. Render a `512x512` RGBA image.
7. Encode to AVIF and (for `convert`) write into PMTiles.

## Georeferencing Priority

Transforms are resolved in this order:

1. `ModelTransformationTag`
2. `ModelPixelScaleTag` + `ModelTiepointTag`
3. Adjacent world file (`.tfw`, `.TFW`, `.tifw`, `.TIFW`)

CRS is read from `GeoKeyDirectoryTag`; when missing, `--src-crs` is required.

## Tile Selection

- Minimum/base zoom is chosen so one tile edge is at least the largest side of the source union extent.
- This yields 1–4 base tiles in most cases.
- For `convert`, default range is `min_zoom..=min_zoom+3` unless overridden.

## Sampling Rules

## Nearest

- For each output pixel, each source proposes a sample.
- Candidate comes from a 2x2 neighborhood around projected source coordinate.
- Out-of-bounds and nodata are ignored.
- The nearest valid candidate (in source pixel distance) wins.

## Bilinear

- Uses weighted 2x2 interpolation.
- Out-of-bounds and nodata neighbors are ignored; weights are renormalized.
- If all neighbors are invalid, pixel becomes transparent.
- With multiple sources, current policy is: first source in input order that yields a valid bilinear sample wins.

## Nodata

`--nodeta` / `--nodata` supports:

- grayscale value (`0`)
- RGB triplet (`255,255,255`)

Matched nodata pixels are treated as transparent (`alpha=0`) and excluded from interpolation.

