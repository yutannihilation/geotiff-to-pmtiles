//! Integration tests using real TIFF files from the [`image-tiff`] test suite.
//!
//! These tests open TIFF files with both `tiff-compio` and the reference `tiff` crate,
//! then compare dimensions, tag values, and decoded pixel data to verify correctness.
//!
//! Test images are located at `tests/images/` and originate from the
//! [`image-tiff`] crate (<https://github.com/image-rs/image-tiff>), licensed under
//! MIT/Apache-2.0. Some images carry an additional libtiff copyright — see
//! `tests/images/COPYRIGHT` for details.
//!
//! [`image-tiff`]: https://github.com/image-rs/image-tiff

use std::path::{Path, PathBuf};

use tiff::decoder::Decoder;
use tiff_compio::{TiffReader, tag};

/// Path to the test images directory (`tests/images/`).
fn test_images_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("images")
}

/// Open a TIFF with the reference `tiff` crate and return (width, height).
fn reference_dims(path: &Path) -> (u32, u32) {
    let file = std::fs::File::open(path).unwrap();
    let mut decoder = Decoder::new(file).unwrap();
    decoder.dimensions().unwrap()
}

/// Open a TIFF with tiff-compio and verify dimensions match the reference.
async fn verify_dimensions(path: &Path) {
    let file = compio::fs::File::open(path).await.unwrap();
    let reader = TiffReader::new(file).await.unwrap();
    let (compio_w, compio_h) = reader.dimensions().unwrap();

    let (ref_w, ref_h) = reference_dims(path);
    assert_eq!(
        (compio_w, compio_h),
        (ref_w, ref_h),
        "dimension mismatch for {}",
        path.display()
    );
}

/// Open a TIFF with tiff-compio, read all chunks, and verify the assembled image
/// matches the reference `tiff` crate's decoded output.
///
/// Note: tiff-compio returns raw bytes in file byte order, while the reference
/// `tiff` crate decodes to native-endian typed values. We serialize the reference
/// values using the file's byte order so both sides match.
async fn verify_pixel_data(path: &Path) {
    // tiff-compio decode (do this first to get byte order)
    let file = compio::fs::File::open(path).await.unwrap();
    let reader = TiffReader::new(file).await.unwrap();
    let byte_order = reader.byte_order();
    let layout = reader.chunk_layout().unwrap();
    let compio_bytes = reader.read_image(&layout).await.unwrap();

    let is_le = byte_order == tiff_compio::ByteOrder::LittleEndian;

    // Reference decode — serialize to the TIFF file's byte order for comparison
    let file = std::fs::File::open(path).unwrap();
    let mut decoder = Decoder::new(file).unwrap();
    let ref_result = decoder.read_image().unwrap();
    macro_rules! to_file_order {
        ($v:expr, $is_le:expr) => {
            $v.iter()
                .flat_map(|x| {
                    if $is_le {
                        x.to_le_bytes()
                    } else {
                        x.to_be_bytes()
                    }
                })
                .collect()
        };
    }
    let ref_bytes: Vec<u8> = match ref_result {
        tiff::decoder::DecodingResult::U8(v) => v,
        tiff::decoder::DecodingResult::I8(v) => v.iter().map(|x| *x as u8).collect(),
        tiff::decoder::DecodingResult::U16(v) => to_file_order!(v, is_le),
        tiff::decoder::DecodingResult::I16(v) => to_file_order!(v, is_le),
        tiff::decoder::DecodingResult::U32(v) => to_file_order!(v, is_le),
        tiff::decoder::DecodingResult::I32(v) => to_file_order!(v, is_le),
        tiff::decoder::DecodingResult::F32(v) => to_file_order!(v, is_le),
        tiff::decoder::DecodingResult::F64(v) => to_file_order!(v, is_le),
        tiff::decoder::DecodingResult::U64(v) => to_file_order!(v, is_le),
        tiff::decoder::DecodingResult::I64(v) => to_file_order!(v, is_le),
        tiff::decoder::DecodingResult::F16(v) => {
            let bits: Vec<u16> = v.iter().map(|x| x.to_bits()).collect();
            to_file_order!(bits, is_le)
        }
    };

    assert_eq!(
        compio_bytes.len(),
        ref_bytes.len(),
        "byte length mismatch for {}: compio={} ref={}",
        path.display(),
        compio_bytes.len(),
        ref_bytes.len(),
    );
    assert_eq!(
        compio_bytes,
        ref_bytes,
        "pixel data mismatch for {}",
        path.display()
    );
}

// ============================================================================
// Dimension-only tests (works for any supported format)
// ============================================================================

#[compio::test]
async fn dimensions_rgb_8b_strip() {
    let path = test_images_dir().join("rgb-3c-8b.tiff");
    assert!(path.exists(), "fixture missing: {}", path.display());
    verify_dimensions(&path).await;
}

#[compio::test]
async fn dimensions_minisblack_8b() {
    let path = test_images_dir().join("minisblack-1c-8b.tiff");
    assert!(path.exists(), "fixture missing: {}", path.display());
    verify_dimensions(&path).await;
}

#[compio::test]
async fn dimensions_minisblack_16b() {
    let path = test_images_dir().join("minisblack-1c-16b.tiff");
    assert!(path.exists(), "fixture missing: {}", path.display());
    verify_dimensions(&path).await;
}

#[compio::test]
async fn dimensions_tiled_rgb_u8() {
    let path = test_images_dir().join("tiled-rgb-u8.tif");
    assert!(path.exists(), "fixture missing: {}", path.display());
    verify_dimensions(&path).await;
}

#[compio::test]
async fn dimensions_lzw() {
    let path = test_images_dir().join("quad-lzw-compat.tiff");
    assert!(path.exists(), "fixture missing: {}", path.display());
    verify_dimensions(&path).await;
}

// ============================================================================
// Full pixel data comparison tests
// ============================================================================

/// Uncompressed, strip-based, 8-bit RGB.
#[compio::test]
async fn pixels_rgb_8b_strip_uncompressed() {
    let path = test_images_dir().join("rgb-3c-8b.tiff");
    assert!(path.exists(), "fixture missing: {}", path.display());
    verify_pixel_data(&path).await;
}

/// Uncompressed, strip-based, 8-bit grayscale.
#[compio::test]
async fn pixels_minisblack_8b() {
    let path = test_images_dir().join("minisblack-1c-8b.tiff");
    assert!(path.exists(), "fixture missing: {}", path.display());
    verify_pixel_data(&path).await;
}

/// Uncompressed, strip-based, 16-bit grayscale.
#[compio::test]
async fn pixels_minisblack_16b() {
    let path = test_images_dir().join("minisblack-1c-16b.tiff");
    assert!(path.exists(), "fixture missing: {}", path.display());
    verify_pixel_data(&path).await;
}

/// Uncompressed, strip-based, 16-bit RGB.
#[compio::test]
async fn pixels_rgb_16b_strip() {
    let path = test_images_dir().join("rgb-3c-16b.tiff");
    assert!(path.exists(), "fixture missing: {}", path.display());
    verify_pixel_data(&path).await;
}

/// Uncompressed, tiled, 8-bit RGB.
#[compio::test]
async fn pixels_tiled_rgb_u8() {
    let path = test_images_dir().join("tiled-rgb-u8.tif");
    assert!(path.exists(), "fixture missing: {}", path.display());
    verify_pixel_data(&path).await;
}

/// Uncompressed, tiled with non-square tiles, 8-bit RGB.
#[compio::test]
async fn pixels_tiled_rect_rgb_u8() {
    let path = test_images_dir().join("tiled-rect-rgb-u8.tif");
    assert!(path.exists(), "fixture missing: {}", path.display());
    verify_pixel_data(&path).await;
}

/// LZW-compressed, strip-based (old-style LSB bit order).
///
/// This file uses LSB bit order for LZW, which the reference `tiff` crate does
/// not support (`read_image` fails with InvalidCode). We verify our decode
/// independently: check that decoding succeeds, output size is correct, and
/// pixel values are plausible (not all zeros).
#[compio::test]
async fn pixels_lzw_strip() {
    let path = test_images_dir().join("quad-lzw-compat.tiff");
    assert!(path.exists(), "fixture missing: {}", path.display());

    let file = compio::fs::File::open(&path).await.unwrap();
    let reader = TiffReader::new(file).await.unwrap();
    let (w, h) = reader.dimensions().unwrap();
    let layout = reader.chunk_layout().unwrap();
    let pixels = reader.read_image(&layout).await.unwrap();

    let expected_len = w as usize * h as usize * layout.bytes_per_pixel;
    assert_eq!(
        pixels.len(),
        expected_len,
        "decoded size mismatch: got {} expected {}",
        pixels.len(),
        expected_len
    );
    // Verify we actually decoded something (not all zeros from padding)
    assert!(
        pixels.iter().any(|&b| b != 0),
        "decoded data is all zeros — likely a decode failure"
    );
}

/// LZW-compressed from issue 69 regression test.
#[compio::test]
async fn pixels_lzw_issue69() {
    let path = test_images_dir().join("issue_69_lzw.tiff");
    assert!(path.exists(), "fixture missing: {}", path.display());
    verify_pixel_data(&path).await;
}

/// Uncompressed, tiled, oversize tiles (tile extends beyond image).
#[compio::test]
async fn pixels_tiled_oversize() {
    let path = test_images_dir().join("tiled-oversize-gray-i8.tif");
    assert!(path.exists(), "fixture missing: {}", path.display());
    verify_pixel_data(&path).await;
}

/// Uncompressed, no RowsPerStrip tag (should default to full image height).
#[compio::test]
async fn pixels_no_rows_per_strip() {
    let path = test_images_dir().join("no_rows_per_strip.tiff");
    assert!(path.exists(), "fixture missing: {}", path.display());
    verify_pixel_data(&path).await;
}

/// Uncompressed, signed 8-bit.
#[compio::test]
async fn pixels_int8() {
    let path = test_images_dir().join("int8.tif");
    assert!(path.exists(), "fixture missing: {}", path.display());
    verify_pixel_data(&path).await;
}

/// Uncompressed, signed 16-bit.
#[compio::test]
async fn pixels_int16() {
    let path = test_images_dir().join("int16.tif");
    assert!(path.exists(), "fixture missing: {}", path.display());
    verify_pixel_data(&path).await;
}

// ============================================================================
// Tag reading tests
// ============================================================================

/// Verify that BitsPerSample tag is correctly read.
#[compio::test]
async fn tag_bits_per_sample_rgb() {
    let path = test_images_dir().join("rgb-3c-8b.tiff");
    assert!(path.exists(), "fixture missing: {}", path.display());
    let file = compio::fs::File::open(&path).await.unwrap();
    let reader = TiffReader::new(file).await.unwrap();
    let bps = reader
        .find_tag(tag::BITS_PER_SAMPLE)
        .unwrap()
        .into_u16_vec()
        .unwrap();
    assert_eq!(bps, vec![8, 8, 8]);
}

/// Verify SamplesPerPixel for an RGB image.
#[compio::test]
async fn tag_samples_per_pixel_rgb() {
    let path = test_images_dir().join("rgb-3c-8b.tiff");
    assert!(path.exists(), "fixture missing: {}", path.display());
    let file = compio::fs::File::open(&path).await.unwrap();
    let reader = TiffReader::new(file).await.unwrap();
    let spp = reader
        .find_tag(tag::SAMPLES_PER_PIXEL)
        .unwrap()
        .into_u16()
        .unwrap();
    assert_eq!(spp, 3);
}

/// Verify Compression tag for an LZW file.
#[compio::test]
async fn tag_compression_lzw() {
    let path = test_images_dir().join("quad-lzw-compat.tiff");
    assert!(path.exists(), "fixture missing: {}", path.display());
    let file = compio::fs::File::open(&path).await.unwrap();
    let reader = TiffReader::new(file).await.unwrap();
    let compression = reader
        .find_tag(tag::COMPRESSION)
        .unwrap()
        .into_u16()
        .unwrap();
    assert_eq!(compression, 5); // LZW
}

/// Verify tile dimensions for a tiled image.
#[compio::test]
async fn tag_tile_dimensions() {
    let path = test_images_dir().join("tiled-rgb-u8.tif");
    assert!(path.exists(), "fixture missing: {}", path.display());
    let file = compio::fs::File::open(&path).await.unwrap();
    let reader = TiffReader::new(file).await.unwrap();

    let tw = reader
        .find_tag(tag::TILE_WIDTH)
        .unwrap()
        .into_u32()
        .unwrap();
    let tl = reader
        .find_tag(tag::TILE_LENGTH)
        .unwrap()
        .into_u32()
        .unwrap();

    // Tile dimensions should be positive and reasonable
    assert!(tw > 0 && tw <= 4096, "unexpected tile width: {tw}");
    assert!(tl > 0 && tl <= 4096, "unexpected tile length: {tl}");
}

// ============================================================================
// Chunk layout tests
// ============================================================================

/// Verify chunk layout for a strip-based image.
#[compio::test]
async fn chunk_layout_strips() {
    let path = test_images_dir().join("rgb-3c-8b.tiff");
    assert!(path.exists(), "fixture missing: {}", path.display());
    let file = compio::fs::File::open(&path).await.unwrap();
    let reader = TiffReader::new(file).await.unwrap();
    let layout = reader.chunk_layout().unwrap();

    assert_eq!(layout.chunk_type, tiff_compio::ChunkType::Strip);
    assert_eq!(layout.chunks_across, 1); // strips span full width
    assert!(layout.chunk_count > 0);
    assert_eq!(layout.offsets.len(), layout.chunk_count as usize);
    assert_eq!(layout.byte_counts.len(), layout.chunk_count as usize);
}

// ============================================================================
// GDAL extension tag tests
// ============================================================================

/// Verify GDAL_NODATA tag (42113) with value "0".
#[compio::test]
async fn tag_gdal_nodata_zero() {
    let path = test_images_dir().join("gdal-nodata-0.tif");
    assert!(path.exists(), "fixture missing: {}", path.display());
    let file = compio::fs::File::open(&path).await.unwrap();
    let reader = TiffReader::new(file).await.unwrap();
    let nodata = reader
        .find_tag(tag::GDAL_NODATA)
        .expect("GDAL_NODATA tag should be present")
        .into_string()
        .unwrap();
    assert_eq!(nodata.trim(), "0");
}

/// Verify GDAL_NODATA tag (42113) with value "255".
#[compio::test]
async fn tag_gdal_nodata_255() {
    let path = test_images_dir().join("gdal-nodata-255.tif");
    assert!(path.exists(), "fixture missing: {}", path.display());
    let file = compio::fs::File::open(&path).await.unwrap();
    let reader = TiffReader::new(file).await.unwrap();
    let nodata = reader
        .find_tag(tag::GDAL_NODATA)
        .expect("GDAL_NODATA tag should be present")
        .into_string()
        .unwrap();
    assert_eq!(nodata.trim(), "255");
}

/// Verify GDAL_NODATA tag is absent from files that don't have it.
#[compio::test]
async fn tag_gdal_nodata_absent() {
    let path = test_images_dir().join("gdal-no-nodata.tif");
    assert!(path.exists(), "fixture missing: {}", path.display());
    let file = compio::fs::File::open(&path).await.unwrap();
    let reader = TiffReader::new(file).await.unwrap();
    assert!(
        reader.find_tag(tag::GDAL_NODATA).is_none(),
        "gdal-no-nodata.tif should not have GDAL_NODATA tag"
    );
}

/// Verify chunk layout for a tiled image.
#[compio::test]
async fn chunk_layout_tiles() {
    let path = test_images_dir().join("tiled-rgb-u8.tif");
    assert!(path.exists(), "fixture missing: {}", path.display());
    let file = compio::fs::File::open(&path).await.unwrap();
    let reader = TiffReader::new(file).await.unwrap();
    let layout = reader.chunk_layout().unwrap();

    assert_eq!(layout.chunk_type, tiff_compio::ChunkType::Tile);
    assert!(layout.chunks_across >= 1);
    assert!(layout.chunks_down >= 1);
    assert_eq!(
        layout.chunk_count,
        layout.chunks_across * layout.chunks_down
    );
    assert_eq!(layout.offsets.len(), layout.chunk_count as usize);
    assert_eq!(layout.byte_counts.len(), layout.chunk_count as usize);
}
