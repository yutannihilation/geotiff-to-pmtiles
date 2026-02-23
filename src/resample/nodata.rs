use super::NoDataSpec;

pub(crate) fn parse_nodeta(
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
                .map_err(|_| format!("invalid --nodeta value: {value}"))?;
            Ok(Some(NoDataSpec::Gray(v)))
        }
        3 => {
            let r: u8 = parts[0]
                .parse()
                .map_err(|_| format!("invalid --nodeta value: {value}"))?;
            let g: u8 = parts[1]
                .parse()
                .map_err(|_| format!("invalid --nodeta value: {value}"))?;
            let b: u8 = parts[2]
                .parse()
                .map_err(|_| format!("invalid --nodeta value: {value}"))?;
            Ok(Some(NoDataSpec::Rgb(r, g, b)))
        }
        _ => Err(format!("invalid --nodeta value: {value}. Use '0' or '255,255,255'.").into()),
    }
}
