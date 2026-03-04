/// TIFF byte order, determined by the first two bytes of the file header.
///
/// - `II` (0x4949) = little-endian (Intel byte order)
/// - `MM` (0x4D4D) = big-endian (Motorola byte order)
///
/// All multi-byte values in the TIFF file (tag values, offsets, counts) must be
/// read according to this byte order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByteOrder {
    /// Intel byte order (`II`): least significant byte first.
    LittleEndian,
    /// Motorola byte order (`MM`): most significant byte first.
    BigEndian,
}

impl ByteOrder {
    /// Read a `u16` from the first 2 bytes of `buf`.
    pub fn read_u16(self, buf: &[u8]) -> u16 {
        let bytes = [buf[0], buf[1]];
        match self {
            ByteOrder::LittleEndian => u16::from_le_bytes(bytes),
            ByteOrder::BigEndian => u16::from_be_bytes(bytes),
        }
    }

    /// Read a `u32` from the first 4 bytes of `buf`.
    pub fn read_u32(self, buf: &[u8]) -> u32 {
        let bytes = [buf[0], buf[1], buf[2], buf[3]];
        match self {
            ByteOrder::LittleEndian => u32::from_le_bytes(bytes),
            ByteOrder::BigEndian => u32::from_be_bytes(bytes),
        }
    }

    /// Read an `i16` from the first 2 bytes of `buf`.
    pub fn read_i16(self, buf: &[u8]) -> i16 {
        let bytes = [buf[0], buf[1]];
        match self {
            ByteOrder::LittleEndian => i16::from_le_bytes(bytes),
            ByteOrder::BigEndian => i16::from_be_bytes(bytes),
        }
    }

    /// Read an `i32` from the first 4 bytes of `buf`.
    pub fn read_i32(self, buf: &[u8]) -> i32 {
        let bytes = [buf[0], buf[1], buf[2], buf[3]];
        match self {
            ByteOrder::LittleEndian => i32::from_le_bytes(bytes),
            ByteOrder::BigEndian => i32::from_be_bytes(bytes),
        }
    }

    /// Read an `f32` from the first 4 bytes of `buf`.
    pub fn read_f32(self, buf: &[u8]) -> f32 {
        let bytes = [buf[0], buf[1], buf[2], buf[3]];
        match self {
            ByteOrder::LittleEndian => f32::from_le_bytes(bytes),
            ByteOrder::BigEndian => f32::from_be_bytes(bytes),
        }
    }

    /// Read an `f64` from the first 8 bytes of `buf`.
    pub fn read_f64(self, buf: &[u8]) -> f64 {
        let bytes = [
            buf[0], buf[1], buf[2], buf[3], buf[4], buf[5], buf[6], buf[7],
        ];
        match self {
            ByteOrder::LittleEndian => f64::from_le_bytes(bytes),
            ByteOrder::BigEndian => f64::from_be_bytes(bytes),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_u16_le() {
        assert_eq!(ByteOrder::LittleEndian.read_u16(&[0x01, 0x02]), 0x0201);
    }

    #[test]
    fn test_read_u16_be() {
        assert_eq!(ByteOrder::BigEndian.read_u16(&[0x01, 0x02]), 0x0102);
    }

    #[test]
    fn test_read_u32_le() {
        assert_eq!(
            ByteOrder::LittleEndian.read_u32(&[0x01, 0x02, 0x03, 0x04]),
            0x04030201
        );
    }

    #[test]
    fn test_read_u32_be() {
        assert_eq!(
            ByteOrder::BigEndian.read_u32(&[0x01, 0x02, 0x03, 0x04]),
            0x01020304
        );
    }

    #[test]
    fn test_read_f64_roundtrip() {
        let val: f64 = 3.14159265358979;
        let le_bytes = val.to_le_bytes();
        assert_eq!(ByteOrder::LittleEndian.read_f64(&le_bytes), val);
        let be_bytes = val.to_be_bytes();
        assert_eq!(ByteOrder::BigEndian.read_f64(&be_bytes), val);
    }

    #[test]
    fn test_read_i16_le() {
        let val: i16 = -1234;
        let bytes = val.to_le_bytes();
        assert_eq!(ByteOrder::LittleEndian.read_i16(&bytes), val);
    }

    #[test]
    fn test_read_f32_roundtrip() {
        let val: f32 = 2.71828;
        let le_bytes = val.to_le_bytes();
        assert_eq!(ByteOrder::LittleEndian.read_f32(&le_bytes), val);
        let be_bytes = val.to_be_bytes();
        assert_eq!(ByteOrder::BigEndian.read_f32(&be_bytes), val);
    }
}
