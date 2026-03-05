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
        (16, 1) => {
            // u16 → u8
            let values: Vec<u16> = raw
                .chunks_exact(2)
                .map(|c| byte_order.read_u16(c))
                .collect();
            normalize_slice(&values, u16::MIN as f64, u16::MAX as f64, |x| *x as f64)
        }
        (32, 1) => {
            // u32 → u8
            let values: Vec<u32> = raw
                .chunks_exact(4)
                .map(|c| byte_order.read_u32(c))
                .collect();
            normalize_slice(&values, u32::MIN as f64, u32::MAX as f64, |x| *x as f64)
        }
        (8, 2) => {
            // i8 → u8
            let values: Vec<i8> = raw.iter().map(|&b| b as i8).collect();
            normalize_slice(&values, i8::MIN as f64, i8::MAX as f64, |x| *x as f64)
        }
        (16, 2) => {
            // i16 → u8
            let values: Vec<i16> = raw
                .chunks_exact(2)
                .map(|c| byte_order.read_i16(c))
                .collect();
            normalize_slice(&values, i16::MIN as f64, i16::MAX as f64, |x| *x as f64)
        }
        (32, 2) => {
            // i32 → u8
            let values: Vec<i32> = raw
                .chunks_exact(4)
                .map(|c| byte_order.read_i32(c))
                .collect();
            normalize_slice(&values, i32::MIN as f64, i32::MAX as f64, |x| *x as f64)
        }
        (32, 3) => {
            // f32 → u8
            let values: Vec<f32> = raw
                .chunks_exact(4)
                .map(|c| byte_order.read_f32(c))
                .collect();
            normalize_slice(&values, f32::MIN as f64, f32::MAX as f64, |x| *x as f64)
        }
        (64, 3) => {
            // f64 → u8
            let values: Vec<f64> = raw
                .chunks_exact(8)
                .map(|c| byte_order.read_f64(c))
                .collect();
            normalize_slice(&values, f64::MIN, f64::MAX, |x| *x)
        }
        _ => raw, // Unknown format — return raw bytes as best effort
    }
}

/// Two-pass min/max normalization to [0, 255].
fn normalize_slice<T, F>(values: &[T], fallback_min: f64, fallback_max: f64, to_f64: F) -> Vec<u8>
where
    F: Fn(&T) -> f64 + Copy,
{
    if values.is_empty() {
        return Vec::new();
    }

    // Pass 1: find actual data range
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    for v in values {
        let n = to_f64(v);
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
        return vec![0; values.len()];
    }

    // Pass 2: normalize to [0, 255]
    values
        .iter()
        .map(|v| {
            let t = ((to_f64(v) - min) / range).clamp(0.0, 1.0);
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
