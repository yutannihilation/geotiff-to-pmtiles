use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;

use rayon::prelude::*;
use tiff::decoder::Decoder;

use super::{SourceMetadata, read_georef};

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn make_temp_dir() -> PathBuf {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time went backwards")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("geotiff_to_pmtiles_inputs_test_{ts}"));
        fs::create_dir_all(&dir).expect("create temp test dir");
        dir
    }

    #[test]
    fn has_glob_meta_detects_meta_chars() {
        assert!(has_glob_meta("*.tif"));
        assert!(has_glob_meta("a?[0-9].tif"));
        assert!(!has_glob_meta("plain.tif"));
    }

    #[test]
    fn resolve_input_paths_accepts_literal_file() {
        let dir = make_temp_dir();
        let p = dir.join("a.tif");
        fs::write(&p, b"x").expect("write file");

        let input = vec![p.to_string_lossy().to_string()];
        let out = resolve_input_paths(&input).expect("resolve literal path");
        assert_eq!(out, vec![p.clone()]);

        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn resolve_input_paths_glob_and_dedup() {
        let dir = make_temp_dir();
        let p1 = dir.join("a.tif");
        let p2 = dir.join("b.tif");
        fs::write(&p1, b"x").expect("write a.tif");
        fs::write(&p2, b"x").expect("write b.tif");

        let glob = dir.join("*.tif").to_string_lossy().to_string();
        let input = vec![glob, p1.to_string_lossy().to_string()];
        let out = resolve_input_paths(&input).expect("resolve glob");

        assert_eq!(out, vec![p1.clone(), p2.clone()]);
        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn resolve_input_paths_reports_errors() {
        let err1 = resolve_input_paths(&[]).expect_err("expected empty-input error");
        assert!(err1.to_string().contains("no input files matched"));

        let literal = vec!["definitely_missing_file.tif".to_string()];
        let err2 = resolve_input_paths(&literal).expect_err("expected missing literal error");
        assert!(err2.to_string().contains("input path matched no files"));

        let pattern = vec!["definitely_missing_*.tif".to_string()];
        let err3 = resolve_input_paths(&pattern).expect_err("expected missing glob error");
        assert!(err3.to_string().contains("glob pattern matched no files"));
    }
}
