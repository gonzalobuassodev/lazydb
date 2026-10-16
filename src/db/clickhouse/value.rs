use crate::db::value::CellValue;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WireColumn {
    pub name: String,
    pub type_name: String,
}

#[cfg(test)]
fn parse_tsv_with_names_and_types(
    bytes: &[u8],
) -> Result<(Vec<WireColumn>, Vec<Vec<CellValue>>), String> {
    if bytes.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| "ClickHouse TSV response is not UTF-8")?;
    let mut lines = text.split_terminator('\n');
    let names = lines
        .next()
        .ok_or("ClickHouse TSV response has no names row")?;
    let types = lines
        .next()
        .ok_or("ClickHouse TSV response has no types row")?;
    let names = split_tsv_row(names.trim_end_matches('\r'))?;
    let types = split_tsv_row(types.trim_end_matches('\r'))?;
    if names.len() != types.len() {
        return Err("ClickHouse TSV names/types column count differs".to_owned());
    }
    let columns = names
        .into_iter()
        .zip(types)
        .map(|(name, type_name)| WireColumn {
            name: decode_tsv(&name),
            type_name: decode_tsv(&type_name),
        })
        .collect::<Vec<_>>();
    let mut rows = Vec::new();
    for line in lines {
        let fields = split_tsv_row(line.trim_end_matches('\r'))?;
        if fields.len() != columns.len() {
            return Err("ClickHouse TSV row column count differs from metadata".to_owned());
        }
        rows.push(
            fields
                .iter()
                .zip(&columns)
                .map(|(value, column)| decode_value(value, &column.type_name))
                .collect(),
        );
    }
    Ok((columns, rows))
}

pub(crate) fn parse_tsv_row(line: &str, columns: &[WireColumn]) -> Result<Vec<CellValue>, String> {
    let fields = split_tsv_row(line.trim_end_matches('\r'))?;
    if fields.len() != columns.len() {
        return Err("ClickHouse TSV row column count differs from metadata".to_owned());
    }
    Ok(fields
        .iter()
        .zip(columns)
        .map(|(value, column)| decode_value(value, &column.type_name))
        .collect())
}

pub(crate) fn parse_names_types_header(
    names: &str,
    types: &str,
) -> Result<Vec<WireColumn>, String> {
    let names = split_tsv_row(names.trim_end_matches('\r'))?;
    let types = split_tsv_row(types.trim_end_matches('\r'))?;
    if names.len() != types.len() {
        return Err("ClickHouse TSV names/types column count differs".to_owned());
    }
    Ok(names
        .into_iter()
        .zip(types)
        .map(|(name, type_name)| WireColumn {
            name: decode_tsv(&name),
            type_name: decode_tsv(&type_name),
        })
        .collect())
}

fn split_tsv_row(line: &str) -> Result<Vec<String>, String> {
    let mut values = Vec::new();
    let mut current = String::new();
    let mut escaped = false;
    for character in line.chars() {
        if escaped {
            current.push('\\');
            current.push(character);
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == '\t' {
            values.push(std::mem::take(&mut current));
        } else {
            current.push(character);
        }
    }
    if escaped {
        return Err("ClickHouse TSV row ends with an incomplete escape".to_owned());
    }
    values.push(current);
    Ok(values)
}

fn decode_value(raw: &str, type_name: &str) -> CellValue {
    if raw == "\\N" && is_nullable_type(type_name) {
        return CellValue::Null;
    }
    let value = decode_tsv(raw);
    let scalar = unwrap_scalar_type(type_name);
    match scalar {
        "Bool" => match value.as_str() {
            "0" | "false" => CellValue::Boolean(false),
            "1" | "true" => CellValue::Boolean(true),
            _ => CellValue::Text(value),
        },
        "Int8" | "Int16" | "Int32" | "Int64" | "UInt8" | "UInt16" | "UInt32" | "UInt64" => {
            if scalar.starts_with("UInt") {
                value
                    .parse::<u64>()
                    .map(CellValue::Unsigned)
                    .unwrap_or(CellValue::Text(value))
            } else {
                value
                    .parse::<i64>()
                    .map(CellValue::Integer)
                    .unwrap_or(CellValue::Text(value))
            }
        }
        "Float32" | "Float64" => value
            .parse::<f64>()
            .ok()
            .filter(|number| number.is_finite())
            .map(CellValue::Float)
            .unwrap_or(CellValue::Text(value)),
        "String" | "FixedString" => CellValue::Text(value),
        "Date" => chrono::NaiveDate::parse_from_str(&value, "%Y-%m-%d")
            .map(CellValue::Date)
            .unwrap_or(CellValue::Text(value)),
        _ => CellValue::Text(value),
    }
}

fn is_nullable_type(mut type_name: &str) -> bool {
    while let Some(inner) = type_name
        .strip_prefix("LowCardinality(")
        .and_then(|inner| inner.strip_suffix(')'))
    {
        type_name = inner;
    }
    type_name.starts_with("Nullable(") && type_name.ends_with(')')
}

fn unwrap_scalar_type(mut type_name: &str) -> &str {
    loop {
        if let Some(inner) = type_name
            .strip_prefix("Nullable(")
            .or_else(|| type_name.strip_prefix("LowCardinality("))
            .and_then(|inner| inner.strip_suffix(')'))
        {
            type_name = inner;
        } else {
            return type_name;
        }
    }
}

fn decode_tsv(value: &str) -> String {
    let mut decoded = String::with_capacity(value.len());
    let mut characters = value.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            decoded.push(character);
            continue;
        }
        match characters.next() {
            Some('t') => decoded.push('\t'),
            Some('n') => decoded.push('\n'),
            Some('r') => decoded.push('\r'),
            Some('0') => decoded.push('\0'),
            Some('\n') => decoded.push('\n'),
            Some('\r') => decoded.push('\r'),
            Some(other) => decoded.push(other),
            None => decoded.push('\\'),
        }
    }
    decoded
}

#[cfg(test)]
mod tests {
    use super::parse_tsv_with_names_and_types;
    use crate::db::value::CellValue;

    #[test]
    fn parses_scalar_nullable_and_escaped_values() {
        let bytes = b"n\ttext\tempty\tmissing\nUInt64\tString\tString\tNullable(String)\n18446744073709551615\tline\\nnext\t\t\\N\n";
        let (columns, rows) = parse_tsv_with_names_and_types(bytes).unwrap();
        assert_eq!(columns[0].name, "n");
        assert_eq!(columns[0].type_name, "UInt64");
        assert_eq!(rows[0][0], CellValue::Unsigned(u64::MAX));
        assert_eq!(rows[0][1], CellValue::Text("line\nnext".into()));
        assert_eq!(rows[0][2], CellValue::Text(String::new()));
        assert_eq!(rows[0][3], CellValue::Null);
    }

    #[test]
    fn names_and_types_header_preserves_zero_row_metadata() {
        let (columns, rows) = parse_tsv_with_names_and_types(b"n\nUInt64\n").unwrap();
        assert_eq!(columns.len(), 1);
        assert_eq!(columns[0].name, "n");
        assert!(rows.is_empty());
    }

    #[test]
    fn parses_multiple_catalog_rows_with_trailing_empty_fields() {
        let bytes =
            b"name\tengine\tcomment\nString\tString\tString\nFoo\tMergeTree\t\nfoo\tMergeTree\t\n";
        let (_, rows) = parse_tsv_with_names_and_types(bytes).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0][0], CellValue::Text("Foo".into()));
        assert_eq!(rows[1][0], CellValue::Text("foo".into()));
        assert_eq!(rows[1][2], CellValue::Text(String::new()));
    }

    #[test]
    fn invalid_tsv_column_framing_is_rejected() {
        assert!(parse_tsv_with_names_and_types(b"a\tb\nString\n").is_err());
        assert!(parse_tsv_with_names_and_types(b"a\nString\n1\t2\n").is_err());
        assert!(parse_tsv_with_names_and_types(b"a\nString\nvalue\\").is_err());
    }

    #[test]
    fn decodes_date_and_keeps_decimal_nan_and_complex_values_as_text() {
        let bytes = b"date\tdecimal\tfloat\tarray\nDate\tDecimal(10, 2)\tFloat64\tArray(UInt8)\n2024-01-02\t123456789012345678.90\tNaN\t[1,2]\n";
        let (_, rows) = parse_tsv_with_names_and_types(bytes).unwrap();
        assert!(matches!(rows[0][0], CellValue::Date(_)));
        assert_eq!(rows[0][1], CellValue::Text("123456789012345678.90".into()));
        assert_eq!(rows[0][2], CellValue::Text("NaN".into()));
        assert_eq!(rows[0][3], CellValue::Text("[1,2]".into()));
    }

    #[test]
    fn escaped_literal_backslash_n_is_distinct_from_nullable_null() {
        let columns = vec![
            super::WireColumn {
                name: "nullable".into(),
                type_name: "Nullable(String)".into(),
            },
            super::WireColumn {
                name: "text".into(),
                type_name: "String".into(),
            },
        ];
        let row = super::parse_tsv_row("\\N\t\\\\N", &columns).unwrap();
        assert_eq!(row[0], CellValue::Null);
        assert_eq!(row[1], CellValue::Text("\\N".into()));
    }

    #[test]
    fn parses_header_escapes() {
        let columns =
            super::parse_names_types_header("line\\tname\tvalue", "String\tUInt8").unwrap();
        assert_eq!(columns[0].name, "line\tname");
        assert_eq!(columns[1].type_name, "UInt8");
    }

    #[test]
    fn every_single_byte_chunk_boundary_preserves_names_types_and_rows() {
        let wire = b"name\tvalue\nString\tNullable(String)\nline\\nnext\t\\N\n";
        for boundary in 0..=wire.len() {
            let first = &wire[..boundary];
            let second = &wire[boundary..];
            let mut lines = Vec::new();
            let mut pending = Vec::new();
            for chunk in [first, second] {
                pending.extend_from_slice(chunk);
                while let Some(index) = pending.iter().position(|byte| *byte == b'\n') {
                    lines.push(pending.drain(..=index).collect::<Vec<_>>());
                }
            }
            assert!(pending.is_empty(), "boundary {boundary}");
            let names = std::str::from_utf8(&lines[0][..lines[0].len() - 1]).unwrap();
            let types = std::str::from_utf8(&lines[1][..lines[1].len() - 1]).unwrap();
            let columns = super::parse_names_types_header(names, types).unwrap();
            let row = super::parse_tsv_row(
                std::str::from_utf8(&lines[2][..lines[2].len() - 1]).unwrap(),
                &columns,
            )
            .unwrap();
            assert_eq!(
                row[0],
                CellValue::Text("line\nnext".into()),
                "boundary {boundary}"
            );
            assert_eq!(row[1], CellValue::Null, "boundary {boundary}");
        }
    }
}
