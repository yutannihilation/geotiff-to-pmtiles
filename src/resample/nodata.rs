use super::NoDataSpec;

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
            let v: u8 = parts[0]
                .parse()
                .map_err(|_| format!("invalid --nodata value: {value}"))?;
            Ok(Some(NoDataSpec::Gray(v)))
        }
        3 => {
            let r: u8 = parts[0]
                .parse()
                .map_err(|_| format!("invalid --nodata value: {value}"))?;
            let g: u8 = parts[1]
                .parse()
                .map_err(|_| format!("invalid --nodata value: {value}"))?;
            let b: u8 = parts[2]
                .parse()
                .map_err(|_| format!("invalid --nodata value: {value}"))?;
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
    fn rejects_invalid_values() {
        assert!(parse_nodata(Some("1,2")).is_err());
        assert!(parse_nodata(Some("x")).is_err());
        assert!(parse_nodata(Some("1,2,300")).is_err());
    }
}
