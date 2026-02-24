use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use image::{ImageBuffer, ImageFormat, Rgba};

const WIDTH: u32 = 4096;
const HEIGHT: u32 = 4096;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out_dir = PathBuf::from("bench-data");
    fs::create_dir_all(&out_dir)?;

    let specs = [
        ("a", -122.70_f64, 37.90_f64, 11_u8),
        ("b", -122.50_f64, 37.75_f64, 29_u8),
        ("c", -122.35_f64, 37.65_f64, 47_u8),
    ];

    for (name, origin_x, origin_y, seed) in specs {
        let tif_path = out_dir.join(format!("{name}.tif"));
        let tfw_path = out_dir.join(format!("{name}.tfw"));
        generate_tiff(&tif_path, seed)?;
        write_tfw(&tfw_path, origin_x, origin_y)?;
        println!("wrote {}", tif_path.display());
        println!("wrote {}", tfw_path.display());
    }

    Ok(())
}

fn generate_tiff(path: &Path, seed: u8) -> Result<(), Box<dyn std::error::Error>> {
    let mut img: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::new(WIDTH, HEIGHT);
    for (x, y, px) in img.enumerate_pixels_mut() {
        let xr = (x as f32 / WIDTH as f32 * 255.0) as u8;
        let yg = (y as f32 / HEIGHT as f32 * 255.0) as u8;
        let wave = (((x ^ y) & 255) as u8).wrapping_add(seed);
        *px = Rgba([xr.wrapping_add(seed), yg, wave, 255]);
    }
    img.save_with_format(path, ImageFormat::Tiff)?;
    Ok(())
}

fn write_tfw(path: &Path, origin_x: f64, origin_y: f64) -> Result<(), Box<dyn std::error::Error>> {
    let mut f = File::create(path)?;
    let pixel_size = 0.00005_f64;
    writeln!(f, "{pixel_size}")?;
    writeln!(f, "0.0")?;
    writeln!(f, "0.0")?;
    writeln!(f, "{}", -pixel_size)?;
    writeln!(f, "{origin_x}")?;
    writeln!(f, "{origin_y}")?;
    Ok(())
}
