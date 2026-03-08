# Changelog

<!-- next-header -->
## [Unreleased] (ReleaseDate)

## [v0.0.12] (2026-03-08)

### New features

- Add `--tile-format` option to choose between AVIF (default) and PNG tile encoding, with `--png-compression` preset (fast/default/best).

## [v0.0.11] (2026-03-06)

### Breaking changes

- Dropped support for planar (PlanarConfiguration=2) and JPEG-compressed TIFFs (#14).

### New features

- Handle GDAL-style nodata values, so pixels marked as nodata are treated as transparent (#13).

### Bug fixes

- Fixed LZW decompression for files using LSB bit order by trying multiple decoder configurations (#14).

<!-- next-url -->
[Unreleased]: https://github.com/yutannihilation/geotiff-to-pmtiles/compare/v0.0.12...HEAD
[v0.0.12]: https://github.com/yutannihilation/geotiff-to-pmtiles/compare/v0.0.11...v0.0.12
[v0.0.11]: https://github.com/yutannihilation/geotiff-to-pmtiles/compare/v0.0.10...v0.0.11
