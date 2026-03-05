# Changelog

## v0.0.11 (unreleased)

### Highlights

- Async I/O with compio: Replaced the `tiff` crate with a custom async TIFF parser (`tiff-compio`) backed by the [compio](https://github.com/compio-rs/compio) runtime (#9, #10, #11). This enables io_uring on Linux and IOCP on Windows for faster tile reads, especially on high-latency storage. This is experimental and may not support all TIFF variants yet.

### Breaking changes

- Dropped support for planar (PlanarConfiguration=2) and JPEG-compressed TIFFs (#14).

### New features

- Handle GDAL-style nodata values, so pixels marked as nodata are treated as transparent (#13).

### Bug fixes

- Fixed LZW decompression for files using LSB bit order by trying multiple decoder configurations (#14).

### Internal

- Removed the lazy load (full-raster fallback) path in favour of the chunked reader (#12).
- Refactored byte-order helpers, chunk layout, and pixel normalization to reduce allocations and clean up abstractions (#11).
