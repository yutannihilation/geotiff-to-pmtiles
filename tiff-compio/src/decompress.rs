//! Decompression dispatch for TIFF chunk data.
//!
//! TIFF supports multiple compression schemes, identified by the `Compression` tag (259).
//! This module handles decompression for the most common schemes:
//!
//! | Value | Name | Crate |
//! |-------|------|-------|
//! | 1 | None | — (passthrough) |
//! | 5 | LZW | [`weezl`] |
//! | 7 | JPEG | [`zune_jpeg`] |
//! | 8, 32946 | Deflate/zlib | [`flate2`] |
//!
//! Decompression is **synchronous** and CPU-bound. It runs after the async read
//! completes, on the same thread. This avoids the overhead of spawning to a thread
//! pool for what is typically a fast, in-memory operation.

use std::io::Read;

use crate::error::TiffError;

/// Decompress raw chunk bytes according to the TIFF compression type.
///
/// # Arguments
///
/// - `data` — raw compressed bytes read from the file.
/// - `compression` — TIFF compression tag value (1, 5, 7, 8, or 32946).
/// - `expected_size` — expected decompressed size in bytes. Used for pre-allocation
///   and for zero-padding short LZW output.
///
/// # Errors
///
/// - [`TiffError::Unsupported`] for unknown compression types.
/// - [`TiffError::Decompress`] if the codec fails.
pub fn decompress(
    data: Vec<u8>,
    compression: u16,
    expected_size: usize,
) -> Result<Vec<u8>, TiffError> {
    match compression {
        1 => {
            // No compression — return the owned buffer directly, no copy needed
            Ok(data)
        }
        5 => {
            // LZW
            decompress_lzw(&data, expected_size)
        }
        8 | 32946 => {
            // Deflate (both old-style 32946 and new-style 8)
            decompress_deflate(&data, expected_size)
        }
        7 => {
            // JPEG
            decompress_jpeg(&data)
        }
        other => Err(TiffError::Unsupported(format!("compression type {other}"))),
    }
}

fn decompress_lzw(data: &[u8], expected_size: usize) -> Result<Vec<u8>, TiffError> {
    let mut decoder = weezl::decode::Decoder::new(weezl::BitOrder::Msb, 8);
    let result = decoder
        .decode(data)
        .map_err(|e| TiffError::Decompress(format!("LZW: {e}")))?;
    if result.len() < expected_size {
        // Pad with zeros if LZW output is short (can happen with padding)
        let mut padded = result;
        padded.resize(expected_size, 0);
        Ok(padded)
    } else {
        Ok(result)
    }
}

fn decompress_deflate(data: &[u8], expected_size: usize) -> Result<Vec<u8>, TiffError> {
    let mut decoder = flate2::read::ZlibDecoder::new(data);
    let mut result = Vec::with_capacity(expected_size);
    decoder
        .read_to_end(&mut result)
        .map_err(|e| TiffError::Decompress(format!("Deflate: {e}")))?;
    Ok(result)
}

fn decompress_jpeg(data: &[u8]) -> Result<Vec<u8>, TiffError> {
    let cursor = std::io::Cursor::new(data);
    let mut decoder = zune_jpeg::JpegDecoder::new(cursor);
    decoder
        .decode()
        .map_err(|e| TiffError::Decompress(format!("JPEG: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_compression() {
        let data = vec![1, 2, 3, 4, 5];
        let result = decompress(data.clone(), 1, 5).unwrap();
        assert_eq!(result, data);
    }

    #[test]
    fn test_lzw_roundtrip() {
        // Compress some data with LZW, then decompress
        let original = vec![0u8; 256]; // repetitive data compresses well
        let mut encoder = weezl::encode::Encoder::new(weezl::BitOrder::Msb, 8);
        let compressed = encoder.encode(&original).unwrap();
        let result = decompress(compressed, 5, 256).unwrap();
        assert_eq!(result, original);
    }

    #[test]
    fn test_deflate_roundtrip() {
        use flate2::write::ZlibEncoder;
        use std::io::Write;

        let original = b"hello world hello world hello world";
        let mut encoder = ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(original).unwrap();
        let compressed = encoder.finish().unwrap();

        let result = decompress(compressed, 8, original.len()).unwrap();
        assert_eq!(result, original);
    }

    #[test]
    fn test_deflate_old_style() {
        use flate2::write::ZlibEncoder;
        use std::io::Write;

        let original = b"test data";
        let mut encoder = ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(original).unwrap();
        let compressed = encoder.finish().unwrap();

        // 32946 is the old-style Deflate tag
        let result = decompress(compressed, 32946, original.len()).unwrap();
        assert_eq!(result, original);
    }

    #[test]
    fn test_unsupported_compression() {
        let result = decompress(vec![0], 9999, 0);
        assert!(result.is_err());
    }
}
