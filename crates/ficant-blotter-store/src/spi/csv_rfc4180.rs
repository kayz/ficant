use std::collections::BTreeMap;

use chrono::{DateTime, NaiveDate, Utc};
use chrono_tz::Tz;
use ficant_application::{ApplicationError, ApplicationErrorCategory, ports::ApplicationResult};
use ficant_domain::primitives::{FixedDecimal, MarketTime};

use crate::codec::parse_fixed_decimal;

/// One parsed RFC-4180 CSV table with validated unique headers and uniform row widths.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CsvTable {
    headers: BTreeMap<String, usize>,
    rows: Vec<Vec<String>>,
}

impl CsvTable {
    /// Parses strict RFC-4180 CSV text (supporting LF and CRLF, quoted fields, escaped `""`, and
    /// embedded commas/newlines inside quoted fields).
    ///
    /// Fails closed (`ValidationFailed`) on empty input, duplicate/empty headers, unclosed quotes,
    /// stray quotes inside unquoted fields, trailing characters after closing quote, or ragged rows.
    pub fn parse(input: &str) -> ApplicationResult<Self> {
        let stripped = input.strip_prefix('\u{feff}').unwrap_or(input);
        let records = parse_rfc4180_records(stripped)?;
        let Some((header_row, data_rows)) = records.split_first() else {
            return Err(validation_failed());
        };
        if header_row.is_empty() {
            return Err(validation_failed());
        }

        let mut headers = BTreeMap::new();
        for (idx, col) in header_row.iter().enumerate() {
            let trimmed = col.trim();
            if trimmed.is_empty() || trimmed != col {
                return Err(validation_failed());
            }
            if headers.insert(trimmed.to_owned(), idx).is_some() {
                return Err(validation_failed());
            }
        }

        let expected_width = header_row.len();
        for row in data_rows {
            if row.len() != expected_width {
                return Err(validation_failed());
            }
        }

        Ok(Self {
            headers,
            rows: data_rows.to_vec(),
        })
    }

    pub fn require_headers(&self, required: &[&str]) -> ApplicationResult<()> {
        for &name in required {
            if !self.headers.contains_key(name) {
                return Err(validation_failed());
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn rows(&self) -> &[Vec<String>] {
        &self.rows
    }

    pub fn field<'a>(&self, row: &'a [String], column: &str) -> ApplicationResult<&'a str> {
        let &idx = self.headers.get(column).ok_or_else(validation_failed)?;
        row.get(idx)
            .map(String::as_str)
            .ok_or_else(validation_failed)
    }

    #[must_use]
    pub fn optional_field<'a>(&self, row: &'a [String], column: &str) -> Option<&'a str> {
        let &idx = self.headers.get(column)?;
        let val = row.get(idx)?.as_str();
        if val.is_empty() { None } else { Some(val) }
    }
}

fn parse_rfc4180_records(input: &str) -> ApplicationResult<Vec<Vec<String>>> {
    if input.is_empty() {
        return Err(validation_failed());
    }

    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut pos = 0_usize;
    let mut records: Vec<Vec<String>> = Vec::new();

    while pos < len {
        let mut row: Vec<String> = Vec::new();
        loop {
            if pos < len && bytes[pos] == b'"' {
                pos += 1;
                let mut field_bytes: Vec<u8> = Vec::new();
                let mut closed = false;
                while pos < len {
                    if bytes[pos] == b'"' {
                        if pos + 1 < len && bytes[pos + 1] == b'"' {
                            field_bytes.push(b'"');
                            pos += 2;
                        } else {
                            pos += 1;
                            closed = true;
                            break;
                        }
                    } else {
                        field_bytes.push(bytes[pos]);
                        pos += 1;
                    }
                }
                if !closed {
                    return Err(validation_failed());
                }
                let field = String::from_utf8(field_bytes).map_err(|_| validation_failed())?;
                row.push(field);

                if pos >= len {
                    break;
                }
                if bytes[pos] == b',' {
                    pos += 1;
                    continue;
                }
                if bytes[pos] == b'\r' {
                    if pos + 1 < len && bytes[pos + 1] == b'\n' {
                        pos += 2;
                        break;
                    }
                    return Err(validation_failed());
                }
                if bytes[pos] == b'\n' {
                    pos += 1;
                    break;
                }
                // Trailing garbage after closing quote -> fail closed.
                return Err(validation_failed());
            }

            // Unquoted field
            let start = pos;
            while pos < len && bytes[pos] != b',' && bytes[pos] != b'\n' && bytes[pos] != b'\r' {
                if bytes[pos] == b'"' {
                    // Bare quote inside unquoted field is illegal in RFC-4180 -> fail closed.
                    return Err(validation_failed());
                }
                pos += 1;
            }
            let field = input[start..pos].to_owned();
            row.push(field);

            if pos >= len {
                break;
            }
            if bytes[pos] == b',' {
                pos += 1;
                continue;
            }
            if bytes[pos] == b'\r' {
                if pos + 1 < len && bytes[pos + 1] == b'\n' {
                    pos += 2;
                    break;
                }
                return Err(validation_failed());
            }
            if bytes[pos] == b'\n' {
                pos += 1;
                break;
            }
        }
        records.push(row);
    }

    Ok(records)
}

pub fn parse_csv_non_empty(value: &str) -> ApplicationResult<String> {
    if value.trim().is_empty() || value != value.trim() {
        return Err(validation_failed());
    }
    Ok(value.to_owned())
}

pub fn parse_csv_u64(value: &str) -> ApplicationResult<u64> {
    if value.trim().is_empty() || value != value.trim() {
        return Err(validation_failed());
    }
    value.parse::<u64>().map_err(|_| validation_failed())
}

pub fn parse_csv_fixed(value: &str) -> ApplicationResult<FixedDecimal> {
    parse_fixed_decimal(value)
}

pub fn parse_csv_date(value: &str) -> ApplicationResult<NaiveDate> {
    if value.trim().is_empty() || value != value.trim() {
        return Err(validation_failed());
    }
    NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| validation_failed())
}

pub fn parse_csv_market_time(rfc3339: &str, tz_name: &str) -> ApplicationResult<MarketTime> {
    let tz: Tz = tz_name.parse().map_err(|_| validation_failed())?;
    let instant = DateTime::parse_from_rfc3339(rfc3339)
        .map_err(|_| validation_failed())?
        .with_timezone(&Utc);
    let local_date = instant.with_timezone(&tz).date_naive();
    MarketTime::new(instant, tz.name(), local_date).map_err(|_| validation_failed())
}

const fn validation_failed() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
}
