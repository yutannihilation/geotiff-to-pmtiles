use super::NoDataSpec;

/// Parse a string as a u8, also accepting float-style strings like `"0.0"` or `"255.0"`
/// by truncating to integer. Returns `None` for values outside the u8 range.
fn parse_u8_lenient(s: &str) -> Option<u8> {
    if let Ok(v) = s.parse::<u8>() {
        return Some(v);
    }
    // Try parsing as float and truncating (handles GDAL-style "0.0", "255.0", etc.)
    if let Ok(f) = s.parse::<f64>()
        && (0.0..=255.0).contains(&f)
        && f.fract() == 0.0
    {
        return Some(f as u8);
    }
    None
}

pub(crate) fn parse_nodata(
    value: Option<&str>,
) -> Result<Option<NoDataSpec>, Box<dyn std::error::Error>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }

    let parts = value
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>();
    match parts.len() {
        1 => {
            let v = parse_u8_lenient(parts[0])
                .ok_or_else(|| format!("invalid --nodata value: {value}"))?;
            Ok(Some(NoDataSpec::Gray(v)))
        }
        3 => {
            let r = parse_u8_lenient(parts[0])
                .ok_or_else(|| format!("invalid --nodata value: {value}"))?;
            let g = parse_u8_lenient(parts[1])
                .ok_or_else(|| format!("invalid --nodata value: {value}"))?;
            let b = parse_u8_lenient(parts[2])
                .ok_or_else(|| format!("invalid --nodata value: {value}"))?;
            Ok(Some(NoDataSpec::Rgb(r, g, b)))
        }
        _ => Err(format!("invalid --nodata value: {value}. Use '0' or '255,255,255'.").into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_empty_as_none() {
        assert!(parse_nodata(None).unwrap().is_none());
        assert!(parse_nodata(Some("")).unwrap().is_none());
        assert!(parse_nodata(Some("   ")).unwrap().is_none());
    }

    #[test]
    fn parses_gray_value() {
        let parsed = parse_nodata(Some("42")).unwrap();
        match parsed {
            Some(NoDataSpec::Gray(v)) => assert_eq!(v, 42),
            _ => panic!("expected gray nodata"),
        }
    }

    #[test]
    fn parses_rgb_value_with_spaces() {
        let parsed = parse_nodata(Some(" 1, 2 ,3 ")).unwrap();
        match parsed {
            Some(NoDataSpec::Rgb(r, g, b)) => assert_eq!((r, g, b), (1, 2, 3)),
            _ => panic!("expected rgb nodata"),
        }
    }

    #[test]
    fn parses_float_style_values() {
        let parsed = parse_nodata(Some("0.0")).unwrap();
        match parsed {
            Some(NoDataSpec::Gray(v)) => assert_eq!(v, 0),
            _ => panic!("expected gray nodata"),
        }

        let parsed = parse_nodata(Some("255.0")).unwrap();
        match parsed {
            Some(NoDataSpec::Gray(v)) => assert_eq!(v, 255),
            _ => panic!("expected gray nodata"),
        }
    }

    #[test]
    fn rejects_invalid_values() {
        assert!(parse_nodata(Some("1,2")).is_err());
        assert!(parse_nodata(Some("x")).is_err());
        assert!(parse_nodata(Some("1,2,300")).is_err());
        assert!(parse_nodata(Some("-9999")).is_err());
        assert!(parse_nodata(Some("0.5")).is_err());
    }
}
