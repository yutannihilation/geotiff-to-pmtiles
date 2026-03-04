//! Async TIFF reader built on [compio](https://github.com/compio-rs/compio).
//!
//! This crate provides [`TiffReader`], an async TIFF file reader that uses compio's
//! completion-based I/O (IOCP on Windows, io_uring on Linux) for efficient file access.
//!
//! # Key design
//!
//! All read operations use [`compio::io::AsyncReadAt`], which provides **position-based**
//! reads (`read_at(&self, buf, offset)`) rather than cursor-based reads. This means:
//!
//! - Every method on [`TiffReader`] takes `&self`, not `&mut self`.
//! - Multiple chunk reads can be issued concurrently from the same file handle without
//!   requiring a mutex or other synchronization.
//!
//! # Supported features
//!
//! - **Byte order:** Little-endian (`II`) and big-endian (`MM`).
//! - **Organization:** Both strip-based and tile-based TIFF images.
//! - **Compression:** None (1), LZW (5), Deflate (8 / 32946), JPEG (7).
//! - **GeoTIFF tags:** ModelTiepoint, ModelPixelScale, ModelTransformation, GeoKeyDirectory.
//!
//! # Usage
//!
//! ```ignore
//! use compio::fs::File;
//! use tiff_compio::{TiffReader, tag};
//!
//! compio::runtime::block_on(async {
//!     // Any type implementing AsyncReadAt works — files, memory buffers, etc.
//!     let file = File::open("image.tif").await.unwrap();
//!     let reader = TiffReader::new(file).await.unwrap();
//!
//!     let (width, height) = reader.dimensions().await.unwrap();
//!     let layout = reader.chunk_layout().await.unwrap();
//!
//!     // Read individual chunks (tiles or strips)
//!     let pixels = reader.read_chunk(&layout, 0).await.unwrap();
//!
//!     // Or read the entire image
//!     let image = reader.read_image(&layout).await.unwrap();
//! });
//! ```

mod byte_order;
mod chunk;
mod decompress;
mod error;
mod header;
mod ifd;
pub mod tag;

pub use byte_order::ByteOrder;
pub use chunk::{ChunkLayout, ChunkType};
pub use error::TiffError;
pub use tag::TagValue;

use compio::io::AsyncReadAt;
use compio::io::AsyncReadAtExt;
use ifd::Ifd;

/// Async TIFF reader built on compio's positional I/O.
///
/// `TiffReader` wraps any [`compio::io::AsyncReadAt`] source and provides methods to
/// parse TIFF metadata (tags, IFD entries) and read pixel data (strips or tiles).
///
/// # Concurrency
///
/// All methods take `&self` since the underlying [`AsyncReadAt::read_at`] is
/// position-based, not cursor-based. This means you can issue multiple concurrent
/// reads from the same `TiffReader` without any locking — a key advantage over
/// cursor-based readers that require `&mut self`.
///
/// # Typical workflow
///
/// 1. Create a reader with [`TiffReader::new`] (reads header + first IFD).
/// 2. Query metadata with [`find_tag`](TiffReader::find_tag) or
///    [`dimensions`](TiffReader::dimensions).
/// 3. Compute the chunk layout once with [`chunk_layout`](TiffReader::chunk_layout).
/// 4. Read pixel data with [`read_chunk`](TiffReader::read_chunk) (individual chunks)
///    or [`read_image`](TiffReader::read_image) (full image).
pub struct TiffReader<R> {
    reader: R,
    byte_order: ByteOrder,
    ifd: Ifd,
}

impl<R: AsyncReadAt> TiffReader<R> {
    /// Open a TIFF file by reading the 8-byte header and parsing the first IFD.
    ///
    /// This performs two or three positional reads:
    /// 1. 8 bytes at offset 0 for the header (byte order, magic number, IFD offset).
    /// 2. 2 bytes at the IFD offset for the entry count.
    /// 3. `N * 12` bytes for the IFD entries + 4 bytes for the next IFD pointer.
    ///
    /// # Errors
    ///
    /// Returns [`TiffError::Format`] if the header is invalid (wrong magic number,
    /// unknown byte order) or the IFD cannot be parsed.
    pub async fn new(reader: R) -> Result<Self, TiffError> {
        let header_buf = read_exact_at(&reader, 0, 8).await?;
        let (byte_order, ifd_offset) = header::parse_header(&header_buf)?;
        let ifd = ifd::read_ifd(&reader, byte_order, ifd_offset).await?;
        Ok(Self {
            reader,
            byte_order,
            ifd,
        })
    }

    /// Look up a tag by its numeric ID.
    ///
    /// Tag IDs are defined as constants in the [`tag`] module (e.g., [`tag::IMAGE_WIDTH`]).
    ///
    /// If the tag's data fits in 4 bytes, it is decoded inline from the IFD entry.
    /// Otherwise, a positional read is issued to fetch the out-of-line data.
    ///
    /// Returns `Ok(None)` if the tag is not present in the IFD.
    pub async fn find_tag(&self, tag_id: u16) -> Result<Option<TagValue>, TiffError> {
        self.ifd
            .resolve_tag(&self.reader, self.byte_order, tag_id)
            .await
    }

    /// Returns the image dimensions as `(width, height)` in pixels.
    ///
    /// Reads from `ImageWidth` (tag 256) and `ImageLength` (tag 257).
    ///
    /// # Errors
    ///
    /// Returns [`TiffError::Format`] if either tag is missing.
    pub async fn dimensions(&self) -> Result<(u32, u32), TiffError> {
        let width = self
            .find_tag(tag::IMAGE_WIDTH)
            .await?
            .ok_or_else(|| TiffError::Format("missing ImageWidth tag".into()))?
            .into_u32()?;
        let height = self
            .find_tag(tag::IMAGE_LENGTH)
            .await?
            .ok_or_else(|| TiffError::Format("missing ImageLength tag".into()))?
            .into_u32()?;
        Ok((width, height))
    }

    /// Parse the chunk layout (strips or tiles) from IFD tags.
    ///
    /// This reads compression, bits-per-sample, samples-per-pixel, and either strip
    /// tags (`StripOffsets`, `RowsPerStrip`, `StripByteCounts`) or tile tags
    /// (`TileWidth`, `TileLength`, `TileOffsets`, `TileByteCounts`).
    ///
    /// The returned [`ChunkLayout`] should be computed once and reused for all
    /// subsequent [`read_chunk`](TiffReader::read_chunk) calls.
    pub async fn chunk_layout(&self) -> Result<ChunkLayout, TiffError> {
        chunk::ChunkLayout::from_reader(self).await
    }

    /// Returns the actual pixel dimensions `(width, height)` of the chunk at `idx`.
    ///
    /// Edge chunks (right column, bottom row) may be smaller than the nominal
    /// chunk size. This method accounts for that.
    pub fn chunk_data_dimensions(layout: &ChunkLayout, idx: u32) -> (u32, u32) {
        layout.chunk_data_dimensions(idx)
    }

    /// Read and decompress a single chunk (strip or tile), returning raw pixel bytes.
    ///
    /// The returned `Vec<u8>` contains decompressed, interleaved pixel data in
    /// row-major order. Its length equals `width * height * samples_per_pixel *
    /// bytes_per_sample`.
    ///
    /// If the chunk's byte count is zero, a zero-filled buffer is returned.
    ///
    /// # Errors
    ///
    /// - [`TiffError::Format`] if `idx >= layout.chunk_count`.
    /// - [`TiffError::Io`] if the positional read fails.
    /// - [`TiffError::Decompress`] if decompression fails.
    pub async fn read_chunk(&self, layout: &ChunkLayout, idx: u32) -> Result<Vec<u8>, TiffError> {
        if idx >= layout.chunk_count {
            return Err(TiffError::Format(format!(
                "chunk index {idx} out of range (count={})",
                layout.chunk_count
            )));
        }

        let offset = layout.offsets[idx as usize];
        let byte_count = layout.byte_counts[idx as usize];

        if byte_count == 0 {
            // Empty chunk — return zeros
            let (w, h) = layout.chunk_data_dimensions(idx);
            let pixel_bytes = w as usize
                * h as usize
                * layout.samples_per_pixel as usize
                * bytes_per_sample(&layout.bits_per_sample);
            return Ok(vec![0u8; pixel_bytes]);
        }

        let compressed = read_exact_at(&self.reader, offset, byte_count as usize).await?;

        let (w, h) = layout.chunk_data_dimensions(idx);
        let expected_size = w as usize
            * h as usize
            * layout.samples_per_pixel as usize
            * bytes_per_sample(&layout.bits_per_sample);

        decompress::decompress(&compressed, layout.compression, expected_size)
    }

    /// Read all chunks and assemble them into a contiguous, full-image pixel buffer.
    ///
    /// The returned buffer is in row-major order with dimensions
    /// `image_width * image_height * samples_per_pixel * bytes_per_sample`.
    ///
    /// Chunks are read sequentially in row-major chunk order (left-to-right,
    /// top-to-bottom) and copied into the correct position in the output buffer.
    ///
    /// For large images, prefer reading individual chunks with
    /// [`read_chunk`](TiffReader::read_chunk) to control memory usage and enable
    /// concurrent processing.
    pub async fn read_image(&self, layout: &ChunkLayout) -> Result<Vec<u8>, TiffError> {
        let bps = bytes_per_sample(&layout.bits_per_sample);
        let row_bytes = layout.image_width as usize * layout.samples_per_pixel as usize * bps;
        let mut image = vec![0u8; layout.image_height as usize * row_bytes];

        for idx in 0..layout.chunk_count {
            let chunk_data = self.read_chunk(layout, idx).await?;
            let (chunk_w, chunk_h) = layout.chunk_data_dimensions(idx);

            let col = (idx % layout.chunks_across) * layout.chunk_width;
            let row = (idx / layout.chunks_across) * layout.chunk_height;

            let chunk_row_bytes = chunk_w as usize * layout.samples_per_pixel as usize * bps;

            for y in 0..chunk_h as usize {
                let img_row = row as usize + y;
                if img_row >= layout.image_height as usize {
                    break;
                }
                let dst_start =
                    img_row * row_bytes + col as usize * layout.samples_per_pixel as usize * bps;
                let src_start = y * chunk_row_bytes;
                let copy_len = chunk_row_bytes
                    .min(row_bytes - col as usize * layout.samples_per_pixel as usize * bps);
                image[dst_start..dst_start + copy_len]
                    .copy_from_slice(&chunk_data[src_start..src_start + copy_len]);
            }
        }

        Ok(image)
    }
}

/// Read exactly `len` bytes at the given file `offset`.
///
/// Allocates a zero-filled `Vec<u8>` of the requested length and uses
/// compio's `read_exact_at` to fill it. The buffer is moved into the kernel
/// for the duration of the I/O operation (owned-buffer pattern).
async fn read_exact_at<R: AsyncReadAt>(
    reader: &R,
    offset: u64,
    len: usize,
) -> Result<Vec<u8>, TiffError> {
    let buf = vec![0u8; len];
    let compio::BufResult(result, buf) = reader.read_exact_at(buf, offset).await;
    result?;
    Ok(buf)
}

/// Compute the number of bytes per sample from the bits-per-sample array.
///
/// Uses the first element of the array (all samples are assumed to have the
/// same bit depth). Returns 1 if the array is empty.
fn bytes_per_sample(bits_per_sample: &[u16]) -> usize {
    if bits_per_sample.is_empty() {
        1
    } else {
        (bits_per_sample[0] as usize).div_ceil(8)
    }
}
