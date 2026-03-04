# tiff-compio

An async TIFF reader built on [compio](https://github.com/compio-rs/compio), a completion-based async I/O library that uses IOCP on Windows and io_uring on Linux.

## Motivation

The standard [`tiff`](https://crates.io/crates/tiff) crate provides synchronous, cursor-based reading (`Read + Seek`), which means:

- A `Decoder` requires `&mut self` for every operation, preventing concurrent chunk reads.
- I/O is blocking and cannot leverage kernel-level async I/O primitives.

`tiff-compio` addresses both issues by building on compio's `AsyncReadAt` trait, which provides **position-based reads** (`read_at(&self, buf, offset)`). This design means:

- All `TiffReader` methods take **`&self`** (not `&mut self`), enabling multiple concurrent chunk reads from the same file handle without a mutex.
- On Windows, I/O dispatches through IOCP; on Linux, through io_uring — both are true completion-based I/O.

## Supported features

| Feature | Status |
|---------|--------|
| Byte order | Little-endian (`II`) and big-endian (`MM`) |
| Organization | Strips and tiles |
| Compression | None (1), LZW (5), Deflate (8 / 32946), JPEG (7) |
| Sample types | `u8`, `u16`, `u32`, `i8`, `i16`, `i32`, `f32`, `f64` |
| GeoTIFF tags | ModelTiepoint, ModelPixelScale, ModelTransformation, GeoKeyDirectory |

**Limitations:**

- Only the first IFD is read (no multi-image / pyramid support yet).
- BigTIFF (64-bit offsets) is not supported.
- Planar configuration is assumed to be chunky (interleaved).

## Usage

```rust
use compio::fs::File;
use tiff_compio::{TiffReader, tag};

compio::runtime::block_on(async {
    let file = File::open("image.tif").await.unwrap();
    let reader = TiffReader::new(file).await.unwrap();

    // Metadata access is synchronous — no .await needed
    let (width, height) = reader.dimensions().unwrap();
    println!("Image size: {width} x {height}");

    // Access GeoTIFF tags (also synchronous)
    if let Some(tiepoint) = reader.find_tag(tag::MODEL_TIEPOINT) {
        let values = tiepoint.into_f64_vec().unwrap();
        println!("Tiepoint: {values:?}");
    }

    // Compute chunk layout once (synchronous)
    let layout = reader.chunk_layout().unwrap();
    for idx in 0..layout.chunk_count {
        let pixels = reader.read_chunk(&layout, idx).await.unwrap();
        let (w, h) = layout.chunk_data_dimensions(idx);
        println!("Chunk {idx}: {w}x{h}, {} bytes", pixels.len());
    }

    // Or read the entire image at once
    let full_image = reader.read_image(&layout).await.unwrap();
    println!("Full image: {} bytes", full_image.len());
});
```

## Architecture

```
tiff-compio/src/
  lib.rs           — TiffReader<R> public API
  error.rs         — TiffError enum
  byte_order.rs    — Endian-aware primitive readers
  header.rs        — 8-byte TIFF header parsing
  tag.rs           — Tag ID constants and TagValue enum
  ifd.rs           — IFD entry parsing and tag value resolution
  chunk.rs         — ChunkLayout (strips/tiles) and spatial indexing
  decompress.rs    — Decompression dispatch (None, LZW, Deflate, JPEG)
```

### Reading pipeline

1. **Header** (8 bytes at offset 0) — determines byte order (LE/BE) and first IFD offset.
2. **IFD** (variable size) — reads entry count, N x 12-byte entries, and next IFD pointer. All tag values are **eagerly resolved** during this step: inline values (≤4 bytes) are decoded from entries, and out-of-line values are fetched via positional reads. Results are stored in a `HashMap<u16, TagValue>` keyed by tag ID.
3. **Tag lookup** — after construction, `find_tag`, `dimensions`, and `chunk_layout` are all **synchronous** `HashMap` lookups with no I/O.
4. **Chunk layout** — computed once from the resolved tags. Unifies strips and tiles into a common `ChunkLayout` structure with offset/bytecount arrays.
5. **Chunk reading** — positional read of compressed bytes, followed by synchronous decompression (LZW/Deflate/JPEG).

### Design decisions

- **`&self` everywhere**: Since `AsyncReadAt::read_at` is position-based, no mutable state is needed for reads. This is the key advantage over cursor-based designs.
- **Decompression is synchronous**: After the async read completes, decompression runs on the current thread. This is intentional — decompression is CPU-bound and benefits from running on the same thread that will process the pixels.
- **`Vec<u8>` owned buffers**: compio uses owned-buffer I/O (the buffer is moved into the kernel and returned on completion). This avoids the borrow-lifetime issues of `io_uring`-style APIs.

## Testing

```bash
cargo test -p tiff-compio
```

Tests use in-memory `Vec<u8>` buffers (which implement `AsyncReadAt`) to construct synthetic TIFF structures, so no fixture files are needed for unit tests.

## Dependencies

| Crate | Purpose |
|-------|---------|
| `compio` | Async runtime with IOCP/io_uring backend |
| `weezl` | LZW decompression |
| `flate2` | Deflate/zlib decompression |
| `zune-jpeg` | JPEG decompression |
| `thiserror` | Error type derivation |
