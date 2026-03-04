//! Print metadata and chunk layout of a TIFF file.
//!
//! Usage:
//!
//! ```sh
//! cargo run --example tiffinfo -- path/to/image.tif
//! ```

use compio::fs::File;
use tiff_compio::{TiffReader, tag};

fn compression_name(code: u16) -> &'static str {
    match code {
        1 => "None",
        5 => "LZW",
        7 => "JPEG",
        8 | 32946 => "Deflate",
        _ => "Unknown",
    }
}

#[compio::main]
async fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("Usage: tiffinfo <file.tif>");
        std::process::exit(1);
    });

    let file = File::open(&path).await.expect("failed to open file");
    let reader = TiffReader::new(file).await.expect("failed to parse TIFF");

    // Dimensions (sync access — tags were resolved during TiffReader::new)
    let (width, height) = reader.dimensions().expect("missing dimension tags");
    println!("Dimensions: {width} x {height}");

    // Samples per pixel
    if let Some(val) = reader.find_tag(tag::SAMPLES_PER_PIXEL) {
        println!("Samples per pixel: {}", val.into_u16().unwrap());
    }

    // Bits per sample
    if let Some(val) = reader.find_tag(tag::BITS_PER_SAMPLE) {
        let bps = val.into_u16_vec().unwrap();
        println!("Bits per sample: {bps:?}");
    }

    // Compression
    if let Some(val) = reader.find_tag(tag::COMPRESSION) {
        let code = val.into_u16().unwrap();
        println!("Compression: {} ({code})", compression_name(code));
    }

    // Chunk layout (strips or tiles)
    let layout = reader.chunk_layout().expect("failed to parse chunk layout");
    println!(
        "Organization: {:?} ({} chunks: {} across x {} down)",
        layout.chunk_type, layout.chunk_count, layout.chunks_across, layout.chunks_down,
    );
    println!(
        "Chunk size: {} x {} pixels",
        layout.chunk_width, layout.chunk_height,
    );

    // GeoTIFF tags
    if let Some(val) = reader.find_tag(tag::MODEL_PIXEL_SCALE) {
        let scale = val.into_f64_vec().unwrap();
        println!("Pixel scale: ({}, {}, {})", scale[0], scale[1], scale[2]);
    }
    if let Some(val) = reader.find_tag(tag::MODEL_TIEPOINT) {
        let tp = val.into_f64_vec().unwrap();
        println!(
            "Tiepoint: raster({}, {}, {}) -> model({}, {}, {})",
            tp[0], tp[1], tp[2], tp[3], tp[4], tp[5],
        );
    }

    // Read first chunk to show data size
    let chunk = reader
        .read_chunk(&layout, 0)
        .await
        .expect("failed to read chunk");
    println!(
        "First chunk: {} bytes decompressed ({} bytes/pixel)",
        chunk.len(),
        layout.bytes_per_pixel,
    );
}
