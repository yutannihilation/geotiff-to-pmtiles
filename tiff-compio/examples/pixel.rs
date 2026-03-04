//! Read and print a single pixel value at a given (x, y) position.
//!
//! Usage:
//!
//! ```sh
//! cargo run --example pixel -- path/to/image.tif 10 20
//! ```

use compio::fs::File;
use tiff_compio::{TiffReader, tag};

fn format_samples(data: &[u8], bps: &[u16]) -> String {
    let mut parts = Vec::with_capacity(bps.len());
    let mut offset = 0;
    for &bits in bps {
        let bytes = bits.div_ceil(8) as usize;
        match bytes {
            1 => {
                parts.push(format!("{}", data[offset]));
                offset += 1;
            }
            2 => {
                let val = u16::from_le_bytes([data[offset], data[offset + 1]]);
                parts.push(format!("{val}"));
                offset += 2;
            }
            4 => {
                let raw = [
                    data[offset],
                    data[offset + 1],
                    data[offset + 2],
                    data[offset + 3],
                ];
                // Could be u32 or f32; show both if it looks like float
                let ival = u32::from_le_bytes(raw);
                let fval = f32::from_le_bytes(raw);
                if fval.is_finite() && bits == 32 {
                    parts.push(format!("{fval}"));
                } else {
                    parts.push(format!("{ival}"));
                }
                offset += 4;
            }
            8 => {
                let raw: [u8; 8] = data[offset..offset + 8].try_into().unwrap();
                let fval = f64::from_le_bytes(raw);
                parts.push(format!("{fval}"));
                offset += 8;
            }
            _ => {
                parts.push(format!("({bits}bit)"));
                offset += bytes;
            }
        }
    }
    parts.join(", ")
}

#[compio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 {
        eprintln!("Usage: pixel <file.tif> <x> <y>");
        std::process::exit(1);
    }
    let path = &args[1];
    let x: u32 = args[2].parse().expect("x must be an integer");
    let y: u32 = args[3].parse().expect("y must be an integer");

    let file = File::open(path).await.expect("failed to open file");
    let reader = TiffReader::new(file).await.expect("failed to parse TIFF");

    let (width, height) = reader.dimensions().expect("missing dimension tags");
    if x >= width || y >= height {
        eprintln!("({x}, {y}) is out of bounds for {width}x{height} image");
        std::process::exit(1);
    }

    let layout = reader.chunk_layout().expect("failed to parse chunk layout");
    let bpp = layout.bytes_per_pixel;

    // Determine which chunk contains (x, y)
    let chunk_col = x / layout.chunk_width;
    let chunk_row = y / layout.chunk_height;
    let chunk_idx = chunk_row * layout.chunks_across + chunk_col;

    // Read and decompress the chunk
    let chunk_data = reader
        .read_chunk(&layout, chunk_idx)
        .await
        .expect("failed to read chunk");

    // Pixel offset within the chunk
    let local_x = x - chunk_col * layout.chunk_width;
    let local_y = y - chunk_row * layout.chunk_height;
    let (chunk_w, _chunk_h) = layout.chunk_data_dimensions(chunk_idx);
    let pixel_offset = (local_y * chunk_w + local_x) as usize * bpp;
    let pixel_bytes = &chunk_data[pixel_offset..pixel_offset + bpp];

    // Print sample format info
    let bps = reader
        .find_tag(tag::BITS_PER_SAMPLE)
        .map(|v| v.into_u16_vec().unwrap())
        .unwrap_or_else(|| vec![8]);
    let spp = reader
        .find_tag(tag::SAMPLES_PER_PIXEL)
        .map(|v| v.into_u16().unwrap())
        .unwrap_or(1);

    let channel_label = match spp {
        1 => "gray",
        3 => "r, g, b",
        4 => "r, g, b, a",
        n => &format!("band0..band{}", n - 1),
    };

    println!("Image: {width}x{height}, {spp} samples/pixel, bits/sample: {bps:?}");
    println!(
        "Pixel ({x}, {y}): [{channel_label}] = [{}]",
        format_samples(pixel_bytes, &bps)
    );
}
