use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use image::{ImageBuffer, ImageFormat, Rgb};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out_dir = PathBuf::from("bench-data");

    generate_small(&out_dir.join("small"))?;
    generate_large(&out_dir.join("large"))?;
    generate_many(&out_dir.join("many"))?;

    Ok(())
}

/// 3 files, 4096×4096 (~64 MB each)
fn generate_small(dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(dir)?;
    let specs = [
        ("a", -122.70_f64, 37.90_f64, 11_u8),
        ("b", -122.50_f64, 37.75_f64, 29_u8),
        ("c", -122.35_f64, 37.65_f64, 47_u8),
    ];
    for (name, ox, oy, seed) in specs {
        generate_tiff(&dir.join(format!("{name}.tif")), 4096, 4096, seed)?;
        write_tfw(&dir.join(format!("{name}.tfw")), ox, oy, 0.00005)?;
        println!("wrote {dir}/{name}.tif", dir = dir.display());
    }
    Ok(())
}

/// 4 files, 16384×16384 (~768 MB each, ~3 GB total)
fn generate_large(dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(dir)?;
    let specs = [
        ("a", -122.80_f64, 37.95_f64, 13_u8),
        ("b", -122.40_f64, 37.95_f64, 31_u8),
        ("c", -122.80_f64, 37.55_f64, 59_u8),
        ("d", -122.40_f64, 37.55_f64, 73_u8),
    ];
    for (name, ox, oy, seed) in specs {
        generate_tiff(&dir.join(format!("{name}.tif")), 16384, 16384, seed)?;
        write_tfw(&dir.join(format!("{name}.tfw")), ox, oy, 0.00005)?;
        println!("wrote {dir}/{name}.tif", dir = dir.display());
    }
    Ok(())
}

/// 30 files, 1024×1024 (~4 MB each), tiled in a 6×5 grid
fn generate_many(dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(dir)?;
    let cols = 6;
    let rows = 5;
    let pixel_size = 0.00005_f64;
    let tile_span = 1024.0 * pixel_size; // ~0.0512°
    let base_x = -122.70_f64;
    let base_y = 37.90_f64;

    for row in 0..rows {
        for col in 0..cols {
            let idx = row * cols + col;
            let name = format!("t{idx:02}");
            let ox = base_x + col as f64 * tile_span;
            let oy = base_y - row as f64 * tile_span;
            let seed = (idx * 7 + 3) as u8;
            generate_tiff(&dir.join(format!("{name}.tif")), 1024, 1024, seed)?;
            write_tfw(&dir.join(format!("{name}.tfw")), ox, oy, pixel_size)?;
            println!("wrote {dir}/{name}.tif", dir = dir.display());
        }
    }
    Ok(())
}

fn generate_tiff(
    path: &Path,
    width: u32,
    height: u32,
    seed: u8,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut img: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::new(width, height);
    for (x, y, px) in img.enumerate_pixels_mut() {
        let xr = (x as f32 / width as f32 * 255.0) as u8;
        let yg = (y as f32 / height as f32 * 255.0) as u8;
        let wave = (((x ^ y) & 255) as u8).wrapping_add(seed);
        *px = Rgb([xr.wrapping_add(seed), yg, wave]);
    }
    img.save_with_format(path, ImageFormat::Tiff)?;
    Ok(())
}

fn write_tfw(
    path: &Path,
    origin_x: f64,
    origin_y: f64,
    pixel_size: f64,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut f = File::create(path)?;
    writeln!(f, "{pixel_size}")?;
    writeln!(f, "0.0")?;
    writeln!(f, "0.0")?;
    writeln!(f, "{}", -pixel_size)?;
    writeln!(f, "{origin_x}")?;
    writeln!(f, "{origin_y}")?;
    Ok(())
}
