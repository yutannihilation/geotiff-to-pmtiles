# Changelog

## v0.0.12 (unreleased)

### New features

- Add `--tile-format` option to choose between AVIF (default) and PNG tile encoding, with `--png-compression` preset (fast/default/best).

## v0.0.11

### Breaking changes

- Dropped support for planar (PlanarConfiguration=2) and JPEG-compressed TIFFs (#14).

### New features

- Handle GDAL-style nodata values, so pixels marked as nodata are treated as transparent (#13).

### Bug fixes

- Fixed LZW decompression for files using LSB bit order by trying multiple decoder configurations (#14).
