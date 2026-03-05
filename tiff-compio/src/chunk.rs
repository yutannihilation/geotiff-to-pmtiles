//! Chunk layout abstraction for TIFF strips and tiles.
//!
//! TIFF images store pixel data in "chunks" — either **strips** (full-width horizontal
//! bands) or **tiles** (rectangular sub-images). This module provides [`ChunkLayout`],
//! a unified representation that works for both organizations.
//!
//! # Strips vs Tiles
//!
//! - **Strips**: Each strip spans the full image width. The image is divided into
//!   horizontal bands of `RowsPerStrip` rows. Strips are identified by tags 273
//!   (StripOffsets) and 279 (StripByteCounts).
//!
//! - **Tiles**: Each tile is a rectangular region of `TileWidth × TileLength` pixels.
//!   Tiles are arranged in a grid. The last column/row of tiles may extend beyond the
//!   image boundary (padding). Tiles are identified by tags 324 (TileOffsets) and 325
//!   (TileByteCounts).
//!
//! [`ChunkLayout`] normalizes both into a common grid model with `chunks_across × chunks_down`
//! chunks, each of size `chunk_width × chunk_height` (with edge chunks potentially smaller).

use crate::TiffReader;
use crate::bytes_per_sample;
use crate::error::TiffError;
use crate::tag;

use compio::io::AsyncReadAt;

/// Whether the TIFF uses strip or tile organization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkType {
    /// Strip-based: full-width horizontal bands.
    Strip,
    /// Tile-based: rectangular sub-images in a grid.
    Tile,
}

/// Unified layout information for TIFF strips or tiles.
///
/// Computed once from IFD tags via [`ChunkLayout::from_reader`] and then reused
/// for all chunk read operations. Contains file offsets and byte counts for every
/// chunk, plus the metadata needed to compute each chunk's pixel dimensions.
///
/// # Chunk indexing
///
/// Chunks are indexed in row-major order: chunk 0 is top-left, chunk 1 is one to
/// its right, etc. For strips, `chunks_across` is always 1, so the index equals
/// the strip number.
///
/// ```text
/// Tile layout (2×2 grid):     Strip layout (3 strips):
/// ┌───┬───┐                   ┌─────────┐
/// │ 0 │ 1 │                   │    0    │
/// ├───┼───┤                   ├─────────┤
/// │ 2 │ 3 │                   │    1    │
/// └───┴───┘                   ├─────────┤
///                             │    2    │
///                             └─────────┘
/// ```
#[derive(Debug, Clone)]
pub struct ChunkLayout {
    /// Whether this image uses strips or tiles.
    pub chunk_type: ChunkType,
    /// Full image width in pixels.
    pub image_width: u32,
    /// Full image height in pixels.
    pub image_height: u32,
    /// Nominal chunk width in pixels (= image width for strips).
    pub chunk_width: u32,
    /// Nominal chunk height in pixels (= rows per strip for strips).
    pub chunk_height: u32,
    /// Number of chunks per row (always 1 for strips).
    pub chunks_across: u32,
    /// Number of chunk rows.
    pub chunks_down: u32,
    /// Total number of chunks (`chunks_across * chunks_down`).
    pub chunk_count: u32,
    /// File offset of each chunk's compressed data.
    pub offsets: Vec<u64>,
    /// Compressed byte count of each chunk.
    pub byte_counts: Vec<u64>,
    /// TIFF compression tag value (1=None, 5=LZW, 7=JPEG, 8=Deflate).
    pub compression: u16,
    /// Bits per sample for each channel (e.g., `[8, 8, 8]` for 8-bit RGB).
    pub bits_per_sample: Vec<u16>,
    /// Number of samples (channels) per pixel.
    pub samples_per_pixel: u16,
    /// Bytes per pixel (`samples_per_pixel * ceil(bits_per_sample[0] / 8)`).
    /// Cached to avoid recomputation on every chunk read.
    pub bytes_per_pixel: usize,
    /// TIFF Predictor tag value (1=None, 2=Horizontal differencing).
    pub predictor: u16,
    /// TIFF SampleFormat tag value (1=uint, 2=int, 3=float, 4=undefined).
    pub sample_format: u16,
}

impl ChunkLayout {
    /// Build a `ChunkLayout` by reading strip/tile tags from a [`TiffReader`].
    ///
    /// This is synchronous because all tag values were eagerly resolved during
    /// [`TiffReader::new`].
    ///
    /// Automatically detects strip vs. tile organization based on the presence
    /// of the `TileWidth` tag (322). Falls back to strip mode if tile tags
    /// are absent.
    ///
    /// # Default values
    ///
    /// - `Compression`: defaults to 1 (None) if missing.
    /// - `BitsPerSample`: defaults to `[8]` if missing.
    /// - `SamplesPerPixel`: defaults to 1 if missing.
    /// - `RowsPerStrip`: defaults to `ImageLength` (single strip) if missing.
    pub fn from_reader<R: AsyncReadAt>(reader: &TiffReader<R>) -> Result<Self, TiffError> {
        let (image_width, image_height) = reader.dimensions()?;

        let compression = reader
            .find_tag(tag::COMPRESSION)
            .map(|v| v.into_u16())
            .transpose()?
            .unwrap_or(1); // default: no compression

        let bits_per_sample = reader
            .find_tag(tag::BITS_PER_SAMPLE)
            .map(|v| v.into_u16_vec())
            .transpose()?
            .unwrap_or_else(|| vec![8]);

        let samples_per_pixel = reader
            .find_tag(tag::SAMPLES_PER_PIXEL)
            .map(|v| v.into_u16())
            .transpose()?
            .unwrap_or(1);

        let predictor = reader
            .find_tag(tag::PREDICTOR)
            .map(|v| v.into_u16())
            .transpose()?
            .unwrap_or(1); // default: no predictor

        let sample_format = reader
            .find_tag(tag::SAMPLE_FORMAT)
            .map(|v| v.into_u16_vec())
            .transpose()?
            .and_then(|v| v.first().copied())
            .unwrap_or(1); // default: unsigned integer

        // Determine strip vs tile
        let has_tile_width = reader.find_tag(tag::TILE_WIDTH).is_some();

        if has_tile_width {
            Self::from_tile_tags(
                reader,
                image_width,
                image_height,
                compression,
                bits_per_sample,
                samples_per_pixel,
                predictor,
                sample_format,
            )
        } else {
            Self::from_strip_tags(
                reader,
                image_width,
                image_height,
                compression,
                bits_per_sample,
                samples_per_pixel,
                predictor,
                sample_format,
            )
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn from_tile_tags<R: AsyncReadAt>(
        reader: &TiffReader<R>,
        image_width: u32,
        image_height: u32,
        compression: u16,
        bits_per_sample: Vec<u16>,
        samples_per_pixel: u16,
        predictor: u16,
        sample_format: u16,
    ) -> Result<Self, TiffError> {
        let tile_width = reader
            .find_tag(tag::TILE_WIDTH)
            .ok_or_else(|| TiffError::Format("missing TileWidth".into()))?
            .into_u32()?;
        let tile_height = reader
            .find_tag(tag::TILE_LENGTH)
            .ok_or_else(|| TiffError::Format("missing TileLength".into()))?
            .into_u32()?;
        let offsets = reader
            .find_tag(tag::TILE_OFFSETS)
            .ok_or_else(|| TiffError::Format("missing TileOffsets".into()))?
            .into_u64_vec()?;
        let byte_counts = reader
            .find_tag(tag::TILE_BYTE_COUNTS)
            .ok_or_else(|| TiffError::Format("missing TileByteCounts".into()))?
            .into_u64_vec()?;

        let chunks_across = image_width.div_ceil(tile_width);
        let chunks_down = image_height.div_ceil(tile_height);
        let chunk_count = chunks_across * chunks_down;

        let bytes_per_pixel = samples_per_pixel as usize * bytes_per_sample(&bits_per_sample);

        Ok(Self {
            chunk_type: ChunkType::Tile,
            image_width,
            image_height,
            chunk_width: tile_width,
            chunk_height: tile_height,
            chunks_across,
            chunks_down,
            chunk_count,
            offsets,
            byte_counts,
            compression,
            bits_per_sample,
            samples_per_pixel,
            bytes_per_pixel,
            predictor,
            sample_format,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn from_strip_tags<R: AsyncReadAt>(
        reader: &TiffReader<R>,
        image_width: u32,
        image_height: u32,
        compression: u16,
        bits_per_sample: Vec<u16>,
        samples_per_pixel: u16,
        predictor: u16,
        sample_format: u16,
    ) -> Result<Self, TiffError> {
        let rows_per_strip = reader
            .find_tag(tag::ROWS_PER_STRIP)
            .map(|v| v.into_u32())
            .transpose()?
            .unwrap_or(image_height); // default: single strip

        let offsets = reader
            .find_tag(tag::STRIP_OFFSETS)
            .ok_or_else(|| TiffError::Format("missing StripOffsets".into()))?
            .into_u64_vec()?;
        let byte_counts = reader
            .find_tag(tag::STRIP_BYTE_COUNTS)
            .ok_or_else(|| TiffError::Format("missing StripByteCounts".into()))?
            .into_u64_vec()?;

        let chunks_down = image_height.div_ceil(rows_per_strip);
        let chunk_count = chunks_down; // strips span full width

        let bytes_per_pixel = samples_per_pixel as usize * bytes_per_sample(&bits_per_sample);

        Ok(Self {
            chunk_type: ChunkType::Strip,
            image_width,
            image_height,
            chunk_width: image_width,
            chunk_height: rows_per_strip,
            chunks_across: 1,
            chunks_down,
            chunk_count,
            offsets,
            byte_counts,
            compression,
            bits_per_sample,
            samples_per_pixel,
            bytes_per_pixel,
            predictor,
            sample_format,
        })
    }

    /// Returns the actual pixel dimensions `(width, height)` of the chunk at index `idx`.
    ///
    /// Interior chunks have the full nominal size (`chunk_width × chunk_height`).
    /// Edge chunks on the right column or bottom row may be smaller if the image
    /// dimensions are not an exact multiple of the chunk size.
    pub fn chunk_data_dimensions(&self, idx: u32) -> (u32, u32) {
        let col = idx % self.chunks_across;
        let row = idx / self.chunks_across;

        let w = if (col + 1) * self.chunk_width > self.image_width {
            self.image_width - col * self.chunk_width
        } else {
            self.chunk_width
        };

        let h = if (row + 1) * self.chunk_height > self.image_height {
            self.image_height - row * self.chunk_height
        } else {
            self.chunk_height
        };

        (w, h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunk_data_dimensions_full() {
        let layout = ChunkLayout {
            chunk_type: ChunkType::Tile,
            image_width: 512,
            image_height: 512,
            chunk_width: 256,
            chunk_height: 256,
            chunks_across: 2,
            chunks_down: 2,
            chunk_count: 4,
            offsets: vec![0; 4],
            byte_counts: vec![0; 4],
            compression: 1,
            bits_per_sample: vec![8],
            samples_per_pixel: 3,
            bytes_per_pixel: 3,
            predictor: 1,
            sample_format: 1,
        };
        assert_eq!(layout.chunk_data_dimensions(0), (256, 256));
        assert_eq!(layout.chunk_data_dimensions(1), (256, 256));
        assert_eq!(layout.chunk_data_dimensions(2), (256, 256));
        assert_eq!(layout.chunk_data_dimensions(3), (256, 256));
    }

    #[test]
    fn test_chunk_data_dimensions_edge() {
        let layout = ChunkLayout {
            chunk_type: ChunkType::Tile,
            image_width: 300,
            image_height: 300,
            chunk_width: 256,
            chunk_height: 256,
            chunks_across: 2,
            chunks_down: 2,
            chunk_count: 4,
            offsets: vec![0; 4],
            byte_counts: vec![0; 4],
            compression: 1,
            bits_per_sample: vec![8],
            samples_per_pixel: 3,
            bytes_per_pixel: 3,
            predictor: 1,
            sample_format: 1,
        };
        assert_eq!(layout.chunk_data_dimensions(0), (256, 256));
        assert_eq!(layout.chunk_data_dimensions(1), (44, 256)); // right edge
        assert_eq!(layout.chunk_data_dimensions(2), (256, 44)); // bottom edge
        assert_eq!(layout.chunk_data_dimensions(3), (44, 44)); // corner
    }

    #[test]
    fn test_strip_dimensions() {
        let layout = ChunkLayout {
            chunk_type: ChunkType::Strip,
            image_width: 100,
            image_height: 250,
            chunk_width: 100,
            chunk_height: 100,
            chunks_across: 1,
            chunks_down: 3,
            chunk_count: 3,
            offsets: vec![0; 3],
            byte_counts: vec![0; 3],
            compression: 1,
            bits_per_sample: vec![8],
            samples_per_pixel: 3,
            bytes_per_pixel: 3,
            predictor: 1,
            sample_format: 1,
        };
        assert_eq!(layout.chunk_data_dimensions(0), (100, 100));
        assert_eq!(layout.chunk_data_dimensions(1), (100, 100));
        assert_eq!(layout.chunk_data_dimensions(2), (100, 50)); // last strip
    }
}
