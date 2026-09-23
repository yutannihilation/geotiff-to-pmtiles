# Algorithms and Data Flow

This document describes the current conversion/resampling strategy used by `geotiff-to-pmtiles`.

## Pipeline

1. Expand input path(s): a single file or a glob pattern.
2. Load each raster and georeference transform.
3. Reproject source corners into Web Mercator (`EPSG:3857`).
4. Build tile coverage from union extent.
5. For each tile, build a piecewise-bilinear tile → source raster mapping (see below).
6. Render a `512x512` RGBA image.
7. Encode to AVIF or PNG (selected via `--tile-format`) and write into PMTiles.

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

## Tile → Source Mapping

Web Mercator → source CRS is not affine, so interpolating source pixel positions
from the four tile corners alone is inaccurate: for a `512x512` tile the error
is about 1 output pixel at z8, 4 px at z6 and 15–25 px at z4 (any non-equatorial
location, geographic or projected source CRS alike), shrinking 4x per zoom level.

Each tile therefore gets a `TileMapping`: a `cells x cells` grid whose corners
hold the exact source pixel position (PROJ + inverse geotransform) and inside
which positions are bilinearly interpolated.

- Refinement starts from the 4 corners (`cells = 1`).
- The exact mapping is evaluated at the midpoints that a `2 * cells` grid would
  add; if the interpolation error there exceeds `MAX_REPROJECTION_ERROR_PX`
  (0.125 output pixels, converted to source pixels with the tile's shortest edge
  scale), `cells` doubles and the pass repeats. `cells` is capped at 64.
  The edge scale is a tile-wide average, so where the local scale is smaller
  (e.g. the high-latitude side of a coarse tile) the true error can exceed the
  target by roughly 15%; this is accepted for simplicity.
- Fine zoom levels typically stay at `cells = 1` (9 PROJ calls per tile and
  source); coarse levels use 16–32 cells, up to the cap of 64 (a warning is
  printed when the cap is hit without meeting the target).
- Rendering steps each row additively inside a cell and re-seeds the cursor at
  cell boundaries; overall throughput is within a few percent of the old
  corner-only mapping.
- If a source cannot be mapped for a tile (PROJ error, collapsed tile), the
  source is skipped for that tile and a warning is printed.
- The chunk bounding box for prefetching is taken over all control points.

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

`--nodata` supports:

- grayscale value (`0`)
- RGB triplet (`255,255,255`)

Matched nodata pixels are treated as transparent (`alpha=0`) and excluded from interpolation.

## Unsupported Features

- **Planar TIFF** (`PlanarConfiguration=2`): rejected at load time. Only chunky/interleaved layout is supported.
- **JPEG-in-TIFF** (compression 7): not implemented. Use Deflate or LZW compressed inputs instead.
