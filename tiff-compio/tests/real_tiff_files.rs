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
use tiff::tags::Tag;
use tiff_compio::{TiffReader, tag};

/// Path to the image-tiff test images directory.
fn test_images_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("images")
}

/// Open a TIFF with the reference `tiff` crate and return (width, height, compression, chunk_type).
fn reference_info(path: &Path) -> (u32, u32, u16) {
    let file = std::fs::File::open(path).unwrap();
    let mut decoder = Decoder::new(file).unwrap();
    let (w, h) = decoder.dimensions().unwrap();
    let compression = decoder
        .get_tag(Tag::Compression)
        .ok()
        .and_then(|v| {
            use tiff::decoder::ifd::Value;
            match v {
                Value::Short(v) => Some(v),
                _ => None,
            }
        })
        .unwrap_or(1);
    (w, h, compression)
}

/// Open a TIFF with tiff-compio and verify dimensions match the reference.
async fn verify_dimensions(path: &Path) {
    let file = compio::fs::File::open(path).await.unwrap();
    let reader = TiffReader::new(file).await.unwrap();
    let (compio_w, compio_h) = reader.dimensions().unwrap();

    let (ref_w, ref_h, _) = reference_info(path);
    assert_eq!(
        (compio_w, compio_h),
        (ref_w, ref_h),
        "dimension mismatch for {}",
        path.display()
    );
}

/// Open a TIFF with tiff-compio, read all chunks, and verify the assembled image
/// matches the reference `tiff` crate's decoded output.
async fn verify_pixel_data(path: &Path) {
    // Reference decode
    let file = std::fs::File::open(path).unwrap();
    let mut decoder = Decoder::new(file).unwrap();
    let ref_result = decoder.read_image().unwrap();
    let ref_bytes = match ref_result {
        tiff::decoder::DecodingResult::U8(v) => v,
        tiff::decoder::DecodingResult::U16(v) => {
            // Convert to bytes (native endian) for comparison
            v.iter().flat_map(|x| x.to_ne_bytes()).collect()
        }
        tiff::decoder::DecodingResult::U32(v) => v.iter().flat_map(|x| x.to_ne_bytes()).collect(),
        tiff::decoder::DecodingResult::I8(v) => v.iter().map(|x| *x as u8).collect(),
        tiff::decoder::DecodingResult::I16(v) => v.iter().flat_map(|x| x.to_ne_bytes()).collect(),
        tiff::decoder::DecodingResult::I32(v) => v.iter().flat_map(|x| x.to_ne_bytes()).collect(),
        tiff::decoder::DecodingResult::F32(v) => v.iter().flat_map(|x| x.to_ne_bytes()).collect(),
        tiff::decoder::DecodingResult::F64(v) => v.iter().flat_map(|x| x.to_ne_bytes()).collect(),
        tiff::decoder::DecodingResult::U64(v) => v.iter().flat_map(|x| x.to_ne_bytes()).collect(),
        tiff::decoder::DecodingResult::I64(v) => v.iter().flat_map(|x| x.to_ne_bytes()).collect(),
        tiff::decoder::DecodingResult::F16(v) => {
            // f16 → 2 bytes each, native endian
            v.iter().flat_map(|x| x.to_bits().to_ne_bytes()).collect()
        }
    };

    // tiff-compio decode
    let file = compio::fs::File::open(path).await.unwrap();
    let reader = TiffReader::new(file).await.unwrap();
    let layout = reader.chunk_layout().unwrap();
    let compio_bytes = reader.read_image(&layout).await.unwrap();

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
    if !path.exists() {
        eprintln!("Skipping: {}", path.display());
        return;
    }
    verify_dimensions(&path).await;
}

#[compio::test]
async fn dimensions_minisblack_8b() {
    let path = test_images_dir().join("minisblack-1c-8b.tiff");
    if !path.exists() {
        eprintln!("Skipping: {}", path.display());
        return;
    }
    verify_dimensions(&path).await;
}

#[compio::test]
async fn dimensions_minisblack_16b() {
    let path = test_images_dir().join("minisblack-1c-16b.tiff");
    if !path.exists() {
        eprintln!("Skipping: {}", path.display());
        return;
    }
    verify_dimensions(&path).await;
}

#[compio::test]
async fn dimensions_tiled_rgb_u8() {
    let path = test_images_dir().join("tiled-rgb-u8.tif");
    if !path.exists() {
        eprintln!("Skipping: {}", path.display());
        return;
    }
    verify_dimensions(&path).await;
}

#[compio::test]
async fn dimensions_lzw() {
    let path = test_images_dir().join("quad-lzw-compat.tiff");
    if !path.exists() {
        eprintln!("Skipping: {}", path.display());
        return;
    }
    verify_dimensions(&path).await;
}

#[compio::test]
async fn dimensions_tiled_jpeg() {
    let path = test_images_dir().join("tiled-jpeg-rgb-u8.tif");
    if !path.exists() {
        eprintln!("Skipping: {}", path.display());
        return;
    }
    verify_dimensions(&path).await;
}

// ============================================================================
// Full pixel data comparison tests
// ============================================================================

/// Uncompressed, strip-based, 8-bit RGB.
#[compio::test]
async fn pixels_rgb_8b_strip_uncompressed() {
    let path = test_images_dir().join("rgb-3c-8b.tiff");
    if !path.exists() {
        eprintln!("Skipping: {}", path.display());
        return;
    }
    verify_pixel_data(&path).await;
}

/// Uncompressed, strip-based, 8-bit grayscale.
#[compio::test]
async fn pixels_minisblack_8b() {
    let path = test_images_dir().join("minisblack-1c-8b.tiff");
    if !path.exists() {
        eprintln!("Skipping: {}", path.display());
        return;
    }
    verify_pixel_data(&path).await;
}

/// Uncompressed, strip-based, 16-bit grayscale.
#[compio::test]
async fn pixels_minisblack_16b() {
    let path = test_images_dir().join("minisblack-1c-16b.tiff");
    if !path.exists() {
        eprintln!("Skipping: {}", path.display());
        return;
    }
    verify_pixel_data(&path).await;
}

/// Uncompressed, strip-based, 16-bit RGB.
#[compio::test]
async fn pixels_rgb_16b_strip() {
    let path = test_images_dir().join("rgb-3c-16b.tiff");
    if !path.exists() {
        eprintln!("Skipping: {}", path.display());
        return;
    }
    verify_pixel_data(&path).await;
}

/// Uncompressed, tiled, 8-bit RGB.
#[compio::test]
async fn pixels_tiled_rgb_u8() {
    let path = test_images_dir().join("tiled-rgb-u8.tif");
    if !path.exists() {
        eprintln!("Skipping: {}", path.display());
        return;
    }
    verify_pixel_data(&path).await;
}

/// Uncompressed, tiled with non-square tiles, 8-bit RGB.
#[compio::test]
async fn pixels_tiled_rect_rgb_u8() {
    let path = test_images_dir().join("tiled-rect-rgb-u8.tif");
    if !path.exists() {
        eprintln!("Skipping: {}", path.display());
        return;
    }
    verify_pixel_data(&path).await;
}

/// LZW-compressed, strip-based.
#[compio::test]
async fn pixels_lzw_strip() {
    let path = test_images_dir().join("quad-lzw-compat.tiff");
    if !path.exists() {
        eprintln!("Skipping: {}", path.display());
        return;
    }
    verify_pixel_data(&path).await;
}

/// LZW-compressed from issue 69 regression test.
#[compio::test]
async fn pixels_lzw_issue69() {
    let path = test_images_dir().join("issue_69_lzw.tiff");
    if !path.exists() {
        eprintln!("Skipping: {}", path.display());
        return;
    }
    verify_pixel_data(&path).await;
}

/// Uncompressed, tiled, oversize tiles (tile extends beyond image).
#[compio::test]
async fn pixels_tiled_oversize() {
    let path = test_images_dir().join("tiled-oversize-gray-i8.tif");
    if !path.exists() {
        eprintln!("Skipping: {}", path.display());
        return;
    }
    verify_pixel_data(&path).await;
}

/// Uncompressed, no RowsPerStrip tag (should default to full image height).
#[compio::test]
async fn pixels_no_rows_per_strip() {
    let path = test_images_dir().join("no_rows_per_strip.tiff");
    if !path.exists() {
        eprintln!("Skipping: {}", path.display());
        return;
    }
    verify_pixel_data(&path).await;
}

/// Uncompressed, 8-bit grayscale with alpha.
#[compio::test]
async fn pixels_gray_alpha() {
    let path = test_images_dir().join("minisblack-2c-8b-alpha.tiff");
    if !path.exists() {
        eprintln!("Skipping: {}", path.display());
        return;
    }
    verify_pixel_data(&path).await;
}

/// Uncompressed, signed 8-bit.
#[compio::test]
async fn pixels_int8() {
    let path = test_images_dir().join("int8.tif");
    if !path.exists() {
        eprintln!("Skipping: {}", path.display());
        return;
    }
    verify_pixel_data(&path).await;
}

/// Uncompressed, signed 16-bit.
#[compio::test]
async fn pixels_int16() {
    let path = test_images_dir().join("int16.tif");
    if !path.exists() {
        eprintln!("Skipping: {}", path.display());
        return;
    }
    verify_pixel_data(&path).await;
}

// ============================================================================
// Tag reading tests
// ============================================================================

/// Verify that BitsPerSample tag is correctly read.
#[compio::test]
async fn tag_bits_per_sample_rgb() {
    let path = test_images_dir().join("rgb-3c-8b.tiff");
    if !path.exists() {
        eprintln!("Skipping: {}", path.display());
        return;
    }
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
    if !path.exists() {
        eprintln!("Skipping: {}", path.display());
        return;
    }
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
    if !path.exists() {
        eprintln!("Skipping: {}", path.display());
        return;
    }
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
    if !path.exists() {
        eprintln!("Skipping: {}", path.display());
        return;
    }
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
    if !path.exists() {
        eprintln!("Skipping: {}", path.display());
        return;
    }
    let file = compio::fs::File::open(&path).await.unwrap();
    let reader = TiffReader::new(file).await.unwrap();
    let layout = reader.chunk_layout().unwrap();

    assert_eq!(layout.chunk_type, tiff_compio::ChunkType::Strip);
    assert_eq!(layout.chunks_across, 1); // strips span full width
    assert!(layout.chunk_count > 0);
    assert_eq!(layout.offsets.len(), layout.chunk_count as usize);
    assert_eq!(layout.byte_counts.len(), layout.chunk_count as usize);
}

/// Verify chunk layout for a tiled image.
#[compio::test]
async fn chunk_layout_tiles() {
    let path = test_images_dir().join("tiled-rgb-u8.tif");
    if !path.exists() {
        eprintln!("Skipping: {}", path.display());
        return;
    }
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
