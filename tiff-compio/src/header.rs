//! TIFF file header parsing.
//!
//! Every TIFF file starts with an 8-byte header:
//!
//! | Offset | Size | Description |
//! |--------|------|-------------|
//! | 0 | 2 | Byte order: `II` (little-endian) or `MM` (big-endian) |
//! | 2 | 2 | Magic number: 42 (0x002A) |
//! | 4 | 4 | File offset of the first IFD |

use crate::byte_order::ByteOrder;
use crate::error::TiffError;

/// Parse the 8-byte TIFF header from a buffer.
///
/// Returns `(byte_order, first_ifd_offset)` on success.
///
/// # Errors
///
/// - [`TiffError::Format`] if the buffer is shorter than 8 bytes, the byte order
///   marker is invalid, or the magic number is not 42.
pub fn parse_header(buf: &[u8]) -> Result<(ByteOrder, u32), TiffError> {
    if buf.len() < 8 {
        return Err(TiffError::Format("header too short".into()));
    }

    let byte_order = match (buf[0], buf[1]) {
        (b'I', b'I') => ByteOrder::LittleEndian,
        (b'M', b'M') => ByteOrder::BigEndian,
        _ => return Err(TiffError::Format("invalid byte order marker".into())),
    };

    let magic = byte_order.read_u16(&buf[2..4]);
    if magic != 42 {
        return Err(TiffError::Format(format!(
            "invalid TIFF magic: expected 42, got {magic}"
        )));
    }

    let ifd_offset = byte_order.read_u32(&buf[4..8]);
    Ok((byte_order, ifd_offset))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_le_header() {
        // II, magic=42, IFD offset=8
        let buf = [b'I', b'I', 42, 0, 8, 0, 0, 0];
        let (bo, offset) = parse_header(&buf).unwrap();
        assert_eq!(bo, ByteOrder::LittleEndian);
        assert_eq!(offset, 8);
    }

    #[test]
    fn test_be_header() {
        // MM, magic=42, IFD offset=256
        let buf = [b'M', b'M', 0, 42, 0, 0, 1, 0];
        let (bo, offset) = parse_header(&buf).unwrap();
        assert_eq!(bo, ByteOrder::BigEndian);
        assert_eq!(offset, 256);
    }

    #[test]
    fn test_invalid_magic() {
        let buf = [b'I', b'I', 43, 0, 8, 0, 0, 0];
        assert!(parse_header(&buf).is_err());
    }

    #[test]
    fn test_invalid_byte_order() {
        let buf = [b'X', b'X', 42, 0, 8, 0, 0, 0];
        assert!(parse_header(&buf).is_err());
    }

    #[test]
    fn test_too_short() {
        let buf = [b'I', b'I', 42, 0];
        assert!(parse_header(&buf).is_err());
    }
}
