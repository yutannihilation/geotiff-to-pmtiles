/// Errors that can occur when reading or parsing a TIFF file.
#[derive(Debug, thiserror::Error)]
pub enum TiffError {
    /// An I/O error occurred during a positional read.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// The TIFF structure is malformed (invalid header, missing required tags, etc.).
    #[error("invalid TIFF: {0}")]
    Format(String),

    /// A valid but unsupported TIFF feature was encountered (e.g., unknown compression
    /// type, unknown data type).
    #[error("unsupported: {0}")]
    Unsupported(String),

    /// Decompression of chunk data failed (LZW, Deflate, or JPEG decode error).
    #[error("decompression error: {0}")]
    Decompress(String),
}
