use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;

use rayon::prelude::*;
use tiff::decoder::Decoder;

use super::{SourceDataset, SourceMetadata, load_raster, read_georef};

fn has_glob_meta(arg: &str) -> bool {
    arg.chars().any(|c| matches!(c, '*' | '?' | '[' | ']'))
}

pub(crate) fn resolve_input_paths(
    input: &[String],
) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let mut paths = Vec::new();
    for arg in input {
        // Try glob expansion first so quoted patterns work across shells.
        let mut matched = 0usize;
        for entry in glob::glob(arg)? {
            match entry {
                Ok(path) if path.is_file() => {
                    matched += 1;
                    paths.push(path);
                }
                Ok(_) => {}
                Err(err) => return Err(err.to_string().into()),
            }
        }
        if matched == 0 {
            // If glob expansion found nothing, treat the argument as a literal file path.
            let p = PathBuf::from(arg);
            if p.is_file() {
                paths.push(p);
                continue;
            }
            let kind = if has_glob_meta(arg) {
                "glob pattern"
            } else {
                "input path"
            };
            return Err(format!("{kind} matched no files: {arg}").into());
        }
    }

    if paths.is_empty() {
        return Err("no input files matched".into());
    }

    paths.sort();
    // Dedup handles mixed input forms that may resolve to the same file.
    paths.dedup();
    Ok(paths)
}

pub(crate) fn load_sources(
    input: &[String],
    src_crs: Option<&str>,
) -> Result<Vec<SourceDataset>, Box<dyn std::error::Error>> {
    let paths = resolve_input_paths(input)?;
    load_sources_from_paths(&paths, src_crs)
}

pub(crate) fn load_sources_from_paths(
    paths: &[PathBuf],
    src_crs: Option<&str>,
) -> Result<Vec<SourceDataset>, Box<dyn std::error::Error>> {
    let sources = paths
        .par_iter()
        .map(|path| -> Result<SourceDataset, String> {
            let raster = load_raster(path.as_path())
                .map_err(|e| format!("failed to load raster '{}': {e}", path.display()))?;
            let georef = read_georef(path.as_path(), src_crs)
                .map_err(|e| format!("failed to read georef '{}': {e}", path.display()))?;
            Ok(SourceDataset { raster, georef })
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
    Ok(sources)
}

pub(crate) fn load_source_metadata(
    input: &[String],
    src_crs: Option<&str>,
) -> Result<Vec<SourceMetadata>, Box<dyn std::error::Error>> {
    let paths = resolve_input_paths(input)?;
    let sources = paths
        .par_iter()
        .map(|path| -> Result<SourceMetadata, String> {
            let (width, height) = raster_dimensions(path.as_path())
                .map_err(|e| format!("failed to read raster size '{}': {e}", path.display()))?;
            let georef = read_georef(path.as_path(), src_crs)
                .map_err(|e| format!("failed to read georef '{}': {e}", path.display()))?;
            Ok(SourceMetadata {
                path: path.clone(),
                width,
                height,
                georef,
            })
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
    Ok(sources)
}

fn raster_dimensions(path: &std::path::Path) -> Result<(usize, usize), Box<dyn std::error::Error>> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut decoder = Decoder::new(reader)?;
    let (w, h) = decoder.dimensions()?;
    Ok((w as usize, h as usize))
}
