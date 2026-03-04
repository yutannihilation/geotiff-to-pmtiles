//! Image File Directory (IFD) parsing and tag value resolution.
//!
//! An IFD is a collection of 12-byte entries (tags) that describe one image in the
//! TIFF file. Each entry stores a tag ID, data type, value count, and either the
//! value itself (if it fits in 4 bytes) or a file offset to the value data.
//!
//! # IFD binary layout
//!
//! ```text
//! [2 bytes] entry count (N)
//! [N × 12 bytes] entries:
//!     [2] tag ID
//!     [2] data type (1=BYTE, 2=ASCII, 3=SHORT, 4=LONG, ...)
//!     [4] value count
//!     [4] value/offset (inline if total_size <= 4, else file offset)
//! [4 bytes] offset to next IFD (0 if last)
//! ```

use std::collections::HashMap;

use compio::io::AsyncReadAt;

use crate::byte_order::ByteOrder;
use crate::error::TiffError;
use crate::read_exact_at;
use crate::tag::TagValue;

/// A raw IFD entry parsed from 12 bytes.
///
/// The `value_offset` field contains either the value itself (if the total data
/// size is <= 4 bytes) or a file offset to where the data is stored.
#[derive(Debug, Clone)]
pub struct IfdEntry {
    /// TIFF data type code (1=BYTE, 2=ASCII, 3=SHORT, 4=LONG, 5=RATIONAL, etc.).
    pub data_type: u16,
    /// Number of values of the given data type.
    pub count: u32,
    /// Raw 4-byte value/offset field. Contains inline data if
    /// `type_size(data_type) * count <= 4`, otherwise a u32 file offset.
    pub value_offset: [u8; 4],
}

/// A parsed Image File Directory: a map of tag ID to entry.
///
/// The IFD is read once during [`TiffReader::new`](crate::TiffReader::new) and
/// then queried via [`resolve_tag`](Ifd::resolve_tag) for individual tag values.
#[derive(Debug)]
pub struct Ifd {
    /// Tag entries keyed by tag ID for O(1) lookup.
    pub entries: HashMap<u16, IfdEntry>,
    /// File offset to the next IFD, or 0 if this is the last one.
    #[allow(dead_code)]
    pub next_ifd_offset: u32,
}

/// Returns the size in bytes of one element of the given TIFF data type.
///
/// | Code | Type | Size |
/// |------|------|------|
/// | 1 | BYTE | 1 |
/// | 2 | ASCII | 1 |
/// | 3 | SHORT | 2 |
/// | 4 | LONG | 4 |
/// | 5 | RATIONAL | 8 |
/// | 6 | SBYTE | 1 |
/// | 7 | UNDEFINED | 1 |
/// | 8 | SSHORT | 2 |
/// | 9 | SLONG | 4 |
/// | 10 | SRATIONAL | 8 |
/// | 11 | FLOAT | 4 |
/// | 12 | DOUBLE | 8 |
fn type_size(data_type: u16) -> Option<usize> {
    match data_type {
        1 => Some(1),  // BYTE
        2 => Some(1),  // ASCII
        3 => Some(2),  // SHORT
        4 => Some(4),  // LONG
        5 => Some(8),  // RATIONAL
        6 => Some(1),  // SBYTE
        7 => Some(1),  // UNDEFINED
        8 => Some(2),  // SSHORT
        9 => Some(4),  // SLONG
        10 => Some(8), // SRATIONAL
        11 => Some(4), // FLOAT
        12 => Some(8), // DOUBLE
        _ => None,
    }
}

/// Read and parse an IFD at the given file offset.
///
/// Performs three positional reads:
/// 1. 2 bytes for the entry count.
/// 2. `count * 12` bytes for all entries.
/// 3. 4 bytes for the next IFD offset.
///
/// Returns a populated [`Ifd`] with entries indexed by tag ID.
pub async fn read_ifd<R: AsyncReadAt>(
    reader: &R,
    byte_order: ByteOrder,
    offset: u32,
) -> Result<Ifd, TiffError> {
    // Read entry count (2 bytes)
    let count_buf = read_exact_at(reader, offset as u64, 2).await?;
    let entry_count = byte_order.read_u16(&count_buf) as usize;

    // Read all entries (N × 12 bytes)
    let entries_offset = offset as u64 + 2;
    let entries_buf = read_exact_at(reader, entries_offset, entry_count * 12).await?;

    let mut entries = HashMap::with_capacity(entry_count);
    for i in 0..entry_count {
        let base = i * 12;
        let tag = byte_order.read_u16(&entries_buf[base..]);
        let data_type = byte_order.read_u16(&entries_buf[base + 2..]);
        let count = byte_order.read_u32(&entries_buf[base + 4..]);
        let mut value_offset = [0u8; 4];
        value_offset.copy_from_slice(&entries_buf[base + 8..base + 12]);
        entries.insert(
            tag,
            IfdEntry {
                data_type,
                count,
                value_offset,
            },
        );
    }

    // Read next IFD offset (4 bytes)
    let next_offset_pos = entries_offset + (entry_count * 12) as u64;
    let next_buf = read_exact_at(reader, next_offset_pos, 4).await?;
    let next_ifd_offset = byte_order.read_u32(&next_buf);

    Ok(Ifd {
        entries,
        next_ifd_offset,
    })
}

impl Ifd {
    /// Resolve a tag's value by its ID, reading out-of-line data if needed.
    ///
    /// If the tag's total data size (`type_size * count`) is <= 4 bytes, the value
    /// is decoded inline from the IFD entry. Otherwise, the entry's 4-byte field
    /// is interpreted as a file offset, and the data is fetched via a positional read.
    ///
    /// Returns `Ok(None)` if the tag is not present in this IFD.
    pub async fn resolve_tag<R: AsyncReadAt>(
        &self,
        reader: &R,
        byte_order: ByteOrder,
        tag_id: u16,
    ) -> Result<Option<TagValue>, TiffError> {
        let entry = match self.entries.get(&tag_id) {
            Some(e) => e,
            None => return Ok(None),
        };

        let ts = type_size(entry.data_type).ok_or_else(|| {
            TiffError::Unsupported(format!("unknown TIFF data type {}", entry.data_type))
        })?;
        let total_size = ts * entry.count as usize;

        let raw = if total_size <= 4 {
            // Inline: data is in the value_offset bytes
            entry.value_offset[..total_size].to_vec()
        } else {
            // Out-of-line: value_offset is a file offset
            let offset = byte_order.read_u32(&entry.value_offset) as u64;
            read_exact_at(reader, offset, total_size).await?
        };

        let value = parse_tag_value(byte_order, entry.data_type, entry.count, &raw)?;
        Ok(Some(value))
    }
}

/// Parse raw bytes into a [`TagValue`] based on the TIFF data type code.
///
/// Handles all 12 standard TIFF data types. RATIONAL and SRATIONAL are converted
/// to `f64` by dividing numerator by denominator. A zero denominator yields 0.0.
fn parse_tag_value(
    byte_order: ByteOrder,
    data_type: u16,
    count: u32,
    raw: &[u8],
) -> Result<TagValue, TiffError> {
    let count = count as usize;
    match data_type {
        1 | 7 => {
            // BYTE or UNDEFINED
            Ok(TagValue::U8(raw[..count].to_vec()))
        }
        2 => {
            // ASCII — strip trailing NUL
            let s = String::from_utf8_lossy(&raw[..count]);
            Ok(TagValue::Ascii(s.trim_end_matches('\0').to_string()))
        }
        3 => {
            // SHORT
            let v: Vec<u16> = (0..count)
                .map(|i| byte_order.read_u16(&raw[i * 2..]))
                .collect();
            Ok(TagValue::U16(v))
        }
        4 => {
            // LONG
            let v: Vec<u32> = (0..count)
                .map(|i| byte_order.read_u32(&raw[i * 4..]))
                .collect();
            Ok(TagValue::U32(v))
        }
        5 => {
            // RATIONAL (two LONGs → f64)
            let v: Vec<f64> = (0..count)
                .map(|i| {
                    let num = byte_order.read_u32(&raw[i * 8..]) as f64;
                    let den = byte_order.read_u32(&raw[i * 8 + 4..]) as f64;
                    if den == 0.0 { 0.0 } else { num / den }
                })
                .collect();
            Ok(TagValue::F64(v))
        }
        6 => {
            // SBYTE
            let v: Vec<i8> = raw[..count].iter().map(|&b| b as i8).collect();
            Ok(TagValue::I8(v))
        }
        8 => {
            // SSHORT
            let v: Vec<i16> = (0..count)
                .map(|i| byte_order.read_i16(&raw[i * 2..]))
                .collect();
            Ok(TagValue::I16(v))
        }
        9 => {
            // SLONG
            let v: Vec<i32> = (0..count)
                .map(|i| byte_order.read_i32(&raw[i * 4..]))
                .collect();
            Ok(TagValue::I32(v))
        }
        10 => {
            // SRATIONAL (two SLONGs → f64)
            let v: Vec<f64> = (0..count)
                .map(|i| {
                    let num = byte_order.read_i32(&raw[i * 8..]) as f64;
                    let den = byte_order.read_i32(&raw[i * 8 + 4..]) as f64;
                    if den == 0.0 { 0.0 } else { num / den }
                })
                .collect();
            Ok(TagValue::F64(v))
        }
        11 => {
            // FLOAT
            let v: Vec<f32> = (0..count)
                .map(|i| byte_order.read_f32(&raw[i * 4..]))
                .collect();
            Ok(TagValue::F32(v))
        }
        12 => {
            // DOUBLE
            let v: Vec<f64> = (0..count)
                .map(|i| byte_order.read_f64(&raw[i * 8..]))
                .collect();
            Ok(TagValue::F64(v))
        }
        _ => Err(TiffError::Unsupported(format!(
            "unknown data type {data_type}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal TIFF IFD buffer with the given entries.
    /// Returns (full_buffer, ifd_offset).
    fn build_ifd_buf(byte_order: ByteOrder, entries: &[(u16, u16, u32, [u8; 4])]) -> Vec<u8> {
        let count = entries.len() as u16;
        let mut buf = Vec::new();

        // Entry count (2 bytes)
        match byte_order {
            ByteOrder::LittleEndian => buf.extend_from_slice(&count.to_le_bytes()),
            ByteOrder::BigEndian => buf.extend_from_slice(&count.to_be_bytes()),
        }

        // Entries (12 bytes each)
        for &(tag, dt, cnt, vo) in entries {
            match byte_order {
                ByteOrder::LittleEndian => {
                    buf.extend_from_slice(&tag.to_le_bytes());
                    buf.extend_from_slice(&dt.to_le_bytes());
                    buf.extend_from_slice(&cnt.to_le_bytes());
                }
                ByteOrder::BigEndian => {
                    buf.extend_from_slice(&tag.to_be_bytes());
                    buf.extend_from_slice(&dt.to_be_bytes());
                    buf.extend_from_slice(&cnt.to_be_bytes());
                }
            }
            buf.extend_from_slice(&vo);
        }

        // Next IFD offset = 0
        buf.extend_from_slice(&[0, 0, 0, 0]);
        buf
    }

    #[compio::test]
    async fn test_read_ifd_le() {
        let bo = ByteOrder::LittleEndian;
        // Tag 256 (ImageWidth), SHORT, count=1, value=100
        let mut vo = [0u8; 4];
        vo[0..2].copy_from_slice(&100u16.to_le_bytes());
        let ifd_buf = build_ifd_buf(bo, &[(256, 3, 1, vo)]);

        let ifd = read_ifd(&ifd_buf, bo, 0).await.unwrap();
        assert_eq!(ifd.entries.len(), 1);
        assert!(ifd.entries.contains_key(&256));
        assert_eq!(ifd.next_ifd_offset, 0);
    }

    #[compio::test]
    async fn test_resolve_inline_short() {
        let bo = ByteOrder::LittleEndian;
        let mut vo = [0u8; 4];
        vo[0..2].copy_from_slice(&42u16.to_le_bytes());
        let buf = build_ifd_buf(bo, &[(256, 3, 1, vo)]);

        let ifd = read_ifd(&buf, bo, 0).await.unwrap();
        let val = ifd.resolve_tag(&buf, bo, 256).await.unwrap().unwrap();
        match val {
            TagValue::U16(v) => assert_eq!(v, vec![42]),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[compio::test]
    async fn test_resolve_out_of_line() {
        let bo = ByteOrder::LittleEndian;

        // IFD at offset 0, one entry pointing to out-of-line data at offset 100
        let mut vo = [0u8; 4];
        vo.copy_from_slice(&100u32.to_le_bytes());
        let mut buf = build_ifd_buf(bo, &[(258, 3, 3, vo)]); // 3 SHORTs = 6 bytes > 4

        // Extend buffer to at least offset 106
        buf.resize(106, 0);
        // Write 3 SHORTs at offset 100
        buf[100..102].copy_from_slice(&8u16.to_le_bytes());
        buf[102..104].copy_from_slice(&8u16.to_le_bytes());
        buf[104..106].copy_from_slice(&8u16.to_le_bytes());

        let ifd = read_ifd(&buf, bo, 0).await.unwrap();
        let val = ifd.resolve_tag(&buf, bo, 258).await.unwrap().unwrap();
        match val {
            TagValue::U16(v) => assert_eq!(v, vec![8, 8, 8]),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[compio::test]
    async fn test_missing_tag() {
        let bo = ByteOrder::LittleEndian;
        let mut vo = [0u8; 4];
        vo[0..2].copy_from_slice(&1u16.to_le_bytes());
        let buf = build_ifd_buf(bo, &[(256, 3, 1, vo)]);

        let ifd = read_ifd(&buf, bo, 0).await.unwrap();
        let val = ifd.resolve_tag(&buf, bo, 999).await.unwrap();
        assert!(val.is_none());
    }
}
