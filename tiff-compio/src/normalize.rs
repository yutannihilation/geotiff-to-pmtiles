//! Normalization of raw TIFF sample data to 8-bit unsigned integers.
//!
//! TIFF images can store pixel data in various numeric formats (u8, u16, u32,
//! i8, i16, i32, f32, f64). This module provides [`normalize_to_u8`] to convert
//! any supported format to `u8` for uniform downstream processing.
//!
//! For 8-bit unsigned data, this is a no-op passthrough. For wider or signed
//! types, a two-pass min/max normalization maps the actual data range to [0, 255].

use crate::byte_order::ByteOrder;

/// Normalize raw TIFF chunk bytes to 8-bit unsigned integers.
///
/// Dispatches on `(bits_per_sample, sample_format)` to decode the raw byte
/// buffer using the correct byte order and map values to the [0, 255] range.
///
/// # Arguments
///
/// - `raw` — decompressed chunk bytes (with predictor already applied).
/// - `bits_per_sample` — bits per sample component (8, 16, 32, 64).
/// - `sample_format` — TIFF SampleFormat tag value (1=uint, 2=int, 3=float).
/// - `byte_order` — byte order of the TIFF file.
///
/// # Supported combinations
///
/// | bits | format | Action |
/// |------|--------|--------|
/// | 8 | uint (1) | Passthrough |
/// | 16 | uint (1) | Normalize `u16` range to [0, 255] |
/// | 32 | uint (1) | Normalize `u32` range to [0, 255] |
/// | 8 | int (2) | Normalize `i8` range to [0, 255] |
/// | 16 | int (2) | Normalize `i16` range to [0, 255] |
/// | 32 | int (2) | Normalize `i32` range to [0, 255] |
/// | 32 | float (3) | Normalize `f32` range to [0, 255] |
/// | 64 | float (3) | Normalize `f64` range to [0, 255] |
///
/// Unknown combinations return the raw bytes unchanged (best-effort).
pub fn normalize_to_u8(
    raw: Vec<u8>,
    bits_per_sample: u16,
    sample_format: u16,
    byte_order: ByteOrder,
) -> Vec<u8> {
    match (bits_per_sample, sample_format) {
        (8, 1) | (8, 4) => raw, // u8 passthrough (4 = undefined, treat as u8)
        (16, 1) => normalize_raw(&raw, 2, u16::MIN as f64, u16::MAX as f64, |c| {
            byte_order.read_u16(c) as f64
        }),
        (32, 1) => normalize_raw(&raw, 4, u32::MIN as f64, u32::MAX as f64, |c| {
            byte_order.read_u32(c) as f64
        }),
        (8, 2) => normalize_raw(&raw, 1, i8::MIN as f64, i8::MAX as f64, |c| {
            c[0] as i8 as f64
        }),
        (16, 2) => normalize_raw(&raw, 2, i16::MIN as f64, i16::MAX as f64, |c| {
            byte_order.read_i16(c) as f64
        }),
        (32, 2) => normalize_raw(&raw, 4, i32::MIN as f64, i32::MAX as f64, |c| {
            byte_order.read_i32(c) as f64
        }),
        (32, 3) => normalize_raw(&raw, 4, f32::MIN as f64, f32::MAX as f64, |c| {
            byte_order.read_f32(c) as f64
        }),
        (64, 3) => normalize_raw(&raw, 8, f64::MIN, f64::MAX, |c| byte_order.read_f64(c)),
        _ => raw, // Unknown format — return raw bytes as best effort
    }
}

/// Two-pass min/max normalization operating directly on raw byte chunks.
///
/// Avoids allocating an intermediate typed `Vec<T>` by decoding each element
/// on the fly via the `decode` closure in both passes.
fn normalize_raw(
    raw: &[u8],
    element_size: usize,
    fallback_min: f64,
    fallback_max: f64,
    decode: impl Fn(&[u8]) -> f64,
) -> Vec<u8> {
    let chunks = raw.chunks_exact(element_size);
    let count = chunks.len();
    if count == 0 {
        return Vec::new();
    }

    // Pass 1: find actual data range
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    for c in chunks {
        let n = decode(c);
        if n.is_finite() {
            min = min.min(n);
            max = max.max(n);
        }
    }

    // If data is constant or non-finite, fallback to the type range
    if !min.is_finite() || !max.is_finite() || (max - min).abs() < f64::EPSILON {
        min = fallback_min;
        max = fallback_max;
    }

    let range = (max - min).abs();
    if range < f64::EPSILON {
        return vec![0; count];
    }

    // Pass 2: normalize to [0, 255]
    raw.chunks_exact(element_size)
        .map(|c| {
            let t = ((decode(c) - min) / range).clamp(0.0, 1.0);
            (t * 255.0).round() as u8
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn u8_passthrough() {
        let data = vec![1, 2, 3, 4, 5];
        let result = normalize_to_u8(data.clone(), 8, 1, ByteOrder::LittleEndian);
        assert_eq!(result, data);
    }

    #[test]
    fn u16_normalizes_full_range_le() {
        let values: Vec<u16> = vec![0, 65535];
        let raw: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        let result = normalize_to_u8(raw, 16, 1, ByteOrder::LittleEndian);
        assert_eq!(result, vec![0, 255]);
    }

    #[test]
    fn u16_normalizes_full_range_be() {
        let values: Vec<u16> = vec![0, 65535];
        let raw: Vec<u8> = values.iter().flat_map(|v| v.to_be_bytes()).collect();
        let result = normalize_to_u8(raw, 16, 1, ByteOrder::BigEndian);
        assert_eq!(result, vec![0, 255]);
    }

    #[test]
    fn i16_normalizes_signed() {
        let values: Vec<i16> = vec![-10, 0, 10];
        let raw: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        let result = normalize_to_u8(raw, 16, 2, ByteOrder::LittleEndian);
        assert_eq!(result, vec![0, 128, 255]);
    }

    #[test]
    fn f32_normalizes() {
        let values: Vec<f32> = vec![0.0, 0.5, 1.0];
        let raw: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        let result = normalize_to_u8(raw, 32, 3, ByteOrder::LittleEndian);
        assert_eq!(result, vec![0, 128, 255]);
    }

    #[test]
    fn constant_values_safe() {
        let values: Vec<u16> = vec![5, 5, 5];
        let raw: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        let result = normalize_to_u8(raw, 16, 1, ByteOrder::LittleEndian);
        assert_eq!(result, vec![0, 0, 0]);
    }

    #[test]
    fn empty_input() {
        let result = normalize_to_u8(vec![], 16, 1, ByteOrder::LittleEndian);
        assert!(result.is_empty());
    }
}
