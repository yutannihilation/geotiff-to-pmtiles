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
//!
//! After decompression, a **predictor** may be applied to undo differencing schemes
//! (TIFF tag 317). Predictor 2 (horizontal differencing) is supported for
//! LZW and Deflate compressed data.

use std::io::Read;

use crate::byte_order::ByteOrder;
use crate::error::TiffError;

/// Decompress raw chunk bytes according to the TIFF compression type,
/// then apply the predictor to undo any differencing.
///
/// # Arguments
///
/// - `data` — raw compressed bytes read from the file.
/// - `compression` — TIFF compression tag value (1, 5, 7, 8, or 32946).
/// - `expected_size` — expected decompressed size in bytes. Used for pre-allocation
///   and for zero-padding short LZW output.
/// - `predictor` — TIFF predictor tag value (1=None, 2=Horizontal differencing).
/// - `bytes_per_sample` — bytes per sample component (e.g., 1 for u8, 2 for u16).
/// - `samples_per_pixel` — number of samples (channels) per pixel.
/// - `row_pixels` — number of pixels per row (chunk width).
/// - `byte_order` — byte order of the TIFF file.
///
/// # Errors
///
/// - [`TiffError::Unsupported`] for unknown compression types or predictor values.
/// - [`TiffError::Decompress`] if the codec fails.
#[allow(clippy::too_many_arguments)]
pub fn decompress(
    data: Vec<u8>,
    compression: u16,
    expected_size: usize,
    predictor: u16,
    bytes_per_sample: usize,
    samples_per_pixel: usize,
    row_pixels: u32,
    byte_order: ByteOrder,
) -> Result<Vec<u8>, TiffError> {
    let mut decompressed = match compression {
        1 => {
            // No compression — return the owned buffer directly, no copy needed
            data
        }
        5 => {
            // LZW
            decompress_lzw(&data, expected_size)?
        }
        8 | 32946 => {
            // Deflate (both old-style 32946 and new-style 8)
            decompress_deflate(&data, expected_size)?
        }
        7 => {
            // JPEG — predictor is not applied to JPEG
            return decompress_jpeg(&data);
        }
        other => return Err(TiffError::Unsupported(format!("compression type {other}"))),
    };

    // Apply predictor after decompression (not needed for JPEG or None compression)
    if predictor == 2 && compression != 1 {
        apply_horizontal_predictor(
            &mut decompressed,
            bytes_per_sample,
            samples_per_pixel,
            row_pixels as usize,
            byte_order,
        );
    } else if predictor > 2 {
        return Err(TiffError::Unsupported(format!(
            "predictor type {predictor}"
        )));
    }

    Ok(decompressed)
}

/// Undo horizontal differencing predictor (TIFF Predictor=2).
///
/// After decompression, each sample value (except the first in each row) is stored
/// as the difference from the previous sample of the same channel. This function
/// reverses that by accumulating across each row, per sample component.
fn apply_horizontal_predictor(
    data: &mut [u8],
    bytes_per_sample: usize,
    samples_per_pixel: usize,
    row_pixels: usize,
    byte_order: ByteOrder,
) {
    let pixel_bytes = bytes_per_sample * samples_per_pixel;
    let row_bytes = pixel_bytes * row_pixels;

    if row_pixels <= 1 || pixel_bytes == 0 || row_bytes == 0 {
        return;
    }

    match bytes_per_sample {
        1 => {
            // Fast path for 8-bit samples (most common case) — byte order irrelevant
            for row in data.chunks_exact_mut(row_bytes) {
                for sample in 0..samples_per_pixel {
                    for i in 1..row_pixels {
                        let prev = row[(i - 1) * pixel_bytes + sample];
                        row[i * pixel_bytes + sample] =
                            row[i * pixel_bytes + sample].wrapping_add(prev);
                    }
                }
            }
        }
        2 => {
            // 16-bit samples — read/write in file byte order
            for row in data.chunks_exact_mut(row_bytes) {
                for sample in 0..samples_per_pixel {
                    for i in 1..row_pixels {
                        let prev_off = (i - 1) * pixel_bytes + sample * 2;
                        let curr_off = i * pixel_bytes + sample * 2;
                        if curr_off + 1 < row.len() && prev_off + 1 < row.len() {
                            let prev = byte_order.read_u16(&row[prev_off..]);
                            let curr = byte_order.read_u16(&row[curr_off..]);
                            let result = curr.wrapping_add(prev);
                            byte_order.write_u16(result, &mut row[curr_off..]);
                        }
                    }
                }
            }
        }
        4 => {
            // 32-bit samples (float or int) — read/write in file byte order
            for row in data.chunks_exact_mut(row_bytes) {
                for sample in 0..samples_per_pixel {
                    for i in 1..row_pixels {
                        let prev_off = (i - 1) * pixel_bytes + sample * 4;
                        let curr_off = i * pixel_bytes + sample * 4;
                        if curr_off + 3 < row.len() && prev_off + 3 < row.len() {
                            let prev = byte_order.read_u32(&row[prev_off..]);
                            let curr = byte_order.read_u32(&row[curr_off..]);
                            let result = curr.wrapping_add(prev);
                            byte_order.write_u32(result, &mut row[curr_off..]);
                        }
                    }
                }
            }
        }
        _ => {
            // Generic fallback for arbitrary bytes_per_sample: treat as byte-level differencing
            for row in data.chunks_exact_mut(row_bytes) {
                for sample_byte in 0..pixel_bytes {
                    for i in 1..row_pixels {
                        let prev = row[(i - 1) * pixel_bytes + sample_byte];
                        row[i * pixel_bytes + sample_byte] =
                            row[i * pixel_bytes + sample_byte].wrapping_add(prev);
                    }
                }
            }
        }
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
        let result = decompress(data.clone(), 1, 5, 1, 1, 1, 5, ByteOrder::LittleEndian).unwrap();
        assert_eq!(result, data);
    }

    #[test]
    fn test_lzw_roundtrip() {
        // Compress some data with LZW, then decompress
        let original = vec![0u8; 256];
        let mut encoder = weezl::encode::Encoder::new(weezl::BitOrder::Msb, 8);
        let compressed = encoder.encode(&original).unwrap();
        let result = decompress(compressed, 5, 256, 1, 1, 1, 256, ByteOrder::LittleEndian).unwrap();
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

        let result = decompress(
            compressed,
            8,
            original.len(),
            1,
            1,
            1,
            original.len() as u32,
            ByteOrder::LittleEndian,
        )
        .unwrap();
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
        let result = decompress(
            compressed,
            32946,
            original.len(),
            1,
            1,
            1,
            original.len() as u32,
            ByteOrder::LittleEndian,
        )
        .unwrap();
        assert_eq!(result, original);
    }

    #[test]
    fn test_unsupported_compression() {
        let result = decompress(vec![0], 9999, 0, 1, 1, 1, 0, ByteOrder::LittleEndian);
        assert!(result.is_err());
    }

    #[test]
    fn test_horizontal_predictor_u8_rgb() {
        // 3-channel u8, 4 pixels per row
        // Original pixels: [10,20,30], [40,50,60], [70,80,90], [100,110,120]
        // After differencing:  [10,20,30], [30,30,30], [30,30,30], [30,30,30]
        let mut differenced = vec![10, 20, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30];
        apply_horizontal_predictor(&mut differenced, 1, 3, 4, ByteOrder::LittleEndian);
        assert_eq!(
            differenced,
            vec![10, 20, 30, 40, 50, 60, 70, 80, 90, 100, 110, 120]
        );
    }

    #[test]
    fn test_horizontal_predictor_single_pixel_row() {
        // Single pixel row — no differencing to undo
        let mut data = vec![42, 128, 200];
        apply_horizontal_predictor(&mut data, 1, 3, 1, ByteOrder::LittleEndian);
        assert_eq!(data, vec![42, 128, 200]);
    }
}
