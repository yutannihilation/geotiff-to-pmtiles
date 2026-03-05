//! TIFF tag ID constants and the [`TagValue`] enum for parsed tag data.
//!
//! Tag IDs follow the [TIFF 6.0 specification](https://www.itu.int/itudoc/itu-t/com16/tiff-fx/docs/tiff6.pdf)
//! and [GeoTIFF specification](https://docs.ogc.org/is/19-008r4/19-008r4.html).
//!
//! Use these constants with [`TiffReader::find_tag`](crate::TiffReader::find_tag):
//!
//! ```ignore
//! use tiff_compio::tag;
//! let value = reader.find_tag(tag::IMAGE_WIDTH).await?;
//! ```

use crate::error::TiffError;

// --- Image dimensions ---

/// Tag 256: Number of columns (pixels per row).
pub const IMAGE_WIDTH: u16 = 256;
/// Tag 257: Number of rows.
pub const IMAGE_LENGTH: u16 = 257;

// --- Image format ---

/// Tag 258: Number of bits per sample (e.g., `[8, 8, 8]` for 8-bit RGB).
pub const BITS_PER_SAMPLE: u16 = 258;
/// Tag 259: Compression scheme (1=None, 5=LZW, 7=JPEG, 8=Deflate, 32946=Deflate).
pub const COMPRESSION: u16 = 259;
/// Tag 262: Color space (0=WhiteIsZero, 1=BlackIsZero, 2=RGB, 3=Palette, etc.).
pub const PHOTOMETRIC_INTERPRETATION: u16 = 262;
/// Tag 277: Number of components per pixel (e.g., 3 for RGB, 4 for RGBA).
pub const SAMPLES_PER_PIXEL: u16 = 277;
/// Tag 284: How samples are stored (1=chunky/interleaved, 2=planar).
pub const PLANAR_CONFIGURATION: u16 = 284;
/// Tag 317: Predictor for compression (1=None, 2=Horizontal differencing).
pub const PREDICTOR: u16 = 317;
/// Tag 339: Data type of each sample (1=uint, 2=int, 3=float, 4=undefined).
pub const SAMPLE_FORMAT: u16 = 339;

// --- Strip layout ---

/// Tag 273: File offset of each strip's compressed data.
pub const STRIP_OFFSETS: u16 = 273;
/// Tag 278: Number of rows per strip.
pub const ROWS_PER_STRIP: u16 = 278;
/// Tag 279: Compressed byte count of each strip.
pub const STRIP_BYTE_COUNTS: u16 = 279;

// --- Tile layout ---

/// Tag 322: Tile width in pixels.
pub const TILE_WIDTH: u16 = 322;
/// Tag 323: Tile height in pixels.
pub const TILE_LENGTH: u16 = 323;
/// Tag 324: File offset of each tile's compressed data.
pub const TILE_OFFSETS: u16 = 324;
/// Tag 325: Compressed byte count of each tile.
pub const TILE_BYTE_COUNTS: u16 = 325;

// --- GeoTIFF tags ---

/// Tag 34735: GeoTIFF key directory (array of SHORT values encoding CRS metadata).
pub const GEO_KEY_DIRECTORY: u16 = 34735;
/// Tag 33922: Model tiepoint — maps raster points to model (geographic) coordinates.
///
/// Stored as groups of 6 DOUBLEs: `(I, J, K, X, Y, Z)` where `(I, J, K)` is the
/// raster point and `(X, Y, Z)` is the corresponding model coordinate.
pub const MODEL_TIEPOINT: u16 = 33922;
/// Tag 33550: Model pixel scale — `(ScaleX, ScaleY, ScaleZ)` in model units per pixel.
pub const MODEL_PIXEL_SCALE: u16 = 33550;
/// Tag 34264: 4x4 transformation matrix from raster to model coordinates.
///
/// When present, this tag supersedes ModelTiepoint + ModelPixelScale.
pub const MODEL_TRANSFORMATION: u16 = 34264;

/// A parsed TIFF tag value.
///
/// TIFF tags store typed arrays of values. This enum represents the possible types
/// after parsing. The `into_*` methods provide convenient, type-checked extraction.
///
/// # Type mapping from TIFF data types
///
/// | TIFF type | Code | Rust type | Variant |
/// |-----------|------|-----------|---------|
/// | BYTE | 1 | `u8` | [`U8`](TagValue::U8) |
/// | ASCII | 2 | `String` | [`Ascii`](TagValue::Ascii) |
/// | SHORT | 3 | `u16` | [`U16`](TagValue::U16) |
/// | LONG | 4 | `u32` | [`U32`](TagValue::U32) |
/// | RATIONAL | 5 | `f64` | [`F64`](TagValue::F64) (numerator/denominator) |
/// | SBYTE | 6 | `i8` | [`I8`](TagValue::I8) |
/// | UNDEFINED | 7 | `u8` | [`U8`](TagValue::U8) |
/// | SSHORT | 8 | `i16` | [`I16`](TagValue::I16) |
/// | SLONG | 9 | `i32` | [`I32`](TagValue::I32) |
/// | SRATIONAL | 10 | `f64` | [`F64`](TagValue::F64) (numerator/denominator) |
/// | FLOAT | 11 | `f32` | [`F32`](TagValue::F32) |
/// | DOUBLE | 12 | `f64` | [`F64`](TagValue::F64) |
#[derive(Debug, Clone)]
pub enum TagValue {
    /// Unsigned 8-bit values (TIFF types BYTE and UNDEFINED).
    U8(Vec<u8>),
    /// Unsigned 16-bit values (TIFF type SHORT).
    U16(Vec<u16>),
    /// Unsigned 32-bit values (TIFF type LONG).
    U32(Vec<u32>),
    /// Signed 8-bit values (TIFF type SBYTE).
    I8(Vec<i8>),
    /// Signed 16-bit values (TIFF type SSHORT).
    I16(Vec<i16>),
    /// Signed 32-bit values (TIFF type SLONG).
    I32(Vec<i32>),
    /// 32-bit floating point values (TIFF type FLOAT).
    F32(Vec<f32>),
    /// 64-bit floating point values (TIFF types DOUBLE, RATIONAL, SRATIONAL).
    F64(Vec<f64>),
    /// ASCII string (TIFF type ASCII, trailing NUL stripped).
    Ascii(String),
}

impl TagValue {
    /// Extract a single `u16` value.
    ///
    /// Succeeds if the value is a single-element `U16` or `U8` vector.
    /// Returns [`TiffError::Format`] otherwise.
    pub fn into_u16(self) -> Result<u16, TiffError> {
        match self {
            TagValue::U16(v) if v.len() == 1 => Ok(v[0]),
            TagValue::U8(v) if v.len() == 1 => Ok(v[0] as u16),
            _ => Err(TiffError::Format("expected single u16 value".into())),
        }
    }

    /// Extract a single `u32` value.
    ///
    /// Succeeds if the value is a single-element `U32`, `U16`, or `U8` vector
    /// (widening conversion). Returns [`TiffError::Format`] otherwise.
    pub fn into_u32(self) -> Result<u32, TiffError> {
        match self {
            TagValue::U32(v) if v.len() == 1 => Ok(v[0]),
            TagValue::U16(v) if v.len() == 1 => Ok(v[0] as u32),
            TagValue::U8(v) if v.len() == 1 => Ok(v[0] as u32),
            _ => Err(TiffError::Format("expected single u32 value".into())),
        }
    }

    /// Extract a `Vec<u16>`.
    ///
    /// Accepts `U16` directly, or widens `U8` values.
    pub fn into_u16_vec(self) -> Result<Vec<u16>, TiffError> {
        match self {
            TagValue::U16(v) => Ok(v),
            TagValue::U8(v) => Ok(v.into_iter().map(|b| b as u16).collect()),
            _ => Err(TiffError::Format("expected u16 vector".into())),
        }
    }

    /// Extract a `Vec<u32>`.
    ///
    /// Accepts `U32` directly, or widens `U16` values.
    pub fn into_u32_vec(self) -> Result<Vec<u32>, TiffError> {
        match self {
            TagValue::U32(v) => Ok(v),
            TagValue::U16(v) => Ok(v.into_iter().map(|x| x as u32).collect()),
            _ => Err(TiffError::Format("expected u32 vector".into())),
        }
    }

    /// Extract a `Vec<u64>` by widening `U32` or `U16` values.
    ///
    /// Useful for strip/tile offset and byte count arrays, which may be stored as
    /// either SHORT or LONG but need to be used as 64-bit file offsets.
    pub fn into_u64_vec(self) -> Result<Vec<u64>, TiffError> {
        match self {
            TagValue::U32(v) => Ok(v.into_iter().map(|x| x as u64).collect()),
            TagValue::U16(v) => Ok(v.into_iter().map(|x| x as u64).collect()),
            _ => Err(TiffError::Format(
                "expected u32/u16 vector for u64 conversion".into(),
            )),
        }
    }

    /// Extract a `Vec<f64>`.
    ///
    /// Accepts `F64` directly (from DOUBLE, RATIONAL, SRATIONAL types),
    /// or widens `F32` values.
    pub fn into_f64_vec(self) -> Result<Vec<f64>, TiffError> {
        match self {
            TagValue::F64(v) => Ok(v),
            TagValue::F32(v) => Ok(v.into_iter().map(|f| f as f64).collect()),
            _ => Err(TiffError::Format("expected f64 vector".into())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_into_u16_from_u16() {
        let v = TagValue::U16(vec![42]);
        assert_eq!(v.into_u16().unwrap(), 42);
    }

    #[test]
    fn test_into_u16_from_u8() {
        let v = TagValue::U8(vec![7]);
        assert_eq!(v.into_u16().unwrap(), 7);
    }

    #[test]
    fn test_into_u16_fails_on_multiple() {
        let v = TagValue::U16(vec![1, 2]);
        assert!(v.into_u16().is_err());
    }

    #[test]
    fn test_into_u32_from_u16() {
        let v = TagValue::U16(vec![1000]);
        assert_eq!(v.into_u32().unwrap(), 1000);
    }

    #[test]
    fn test_into_u16_vec() {
        let v = TagValue::U16(vec![8, 8, 8]);
        assert_eq!(v.into_u16_vec().unwrap(), vec![8, 8, 8]);
    }

    #[test]
    fn test_into_u64_vec_from_u32() {
        let v = TagValue::U32(vec![100, 200, 300]);
        assert_eq!(v.into_u64_vec().unwrap(), vec![100u64, 200, 300]);
    }

    #[test]
    fn test_into_f64_vec() {
        let v = TagValue::F64(vec![1.0, 2.5, 3.7]);
        let result = v.into_f64_vec().unwrap();
        assert_eq!(result, vec![1.0, 2.5, 3.7]);
    }

    #[test]
    fn test_into_f64_vec_from_f32() {
        let v = TagValue::F32(vec![1.0f32]);
        let result = v.into_f64_vec().unwrap();
        assert_eq!(result.len(), 1);
    }
}
