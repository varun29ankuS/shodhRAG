//! Tabular document extraction for spreadsheets (via calamine) and delimited
//! text files (CSV/TSV via the `csv` crate).
//!
//! Every sheet / file is normalised into an [`ExtractedTable`]: fully-empty rows
//! and columns are dropped, the first non-empty row becomes the header row, and
//! cell values are rendered the way a person reading the sheet would see them
//! (whole numbers without a trailing `.0`, no binary float noise, dates as ISO
//! `yyyy-mm-dd`). Tables are handed to the chunker as
//! [`DocumentSection::Table`] so rows can be chunked without being split.

use std::path::Path;

use calamine::{open_workbook_auto, Data, ExcelDateTime, Reader};
use chrono::Timelike;
use encoding_rs::Encoding;
use thiserror::Error;

use crate::types::DocumentSection;

/// Errors raised while extracting tables from a file.
#[derive(Debug, Error)]
pub enum TabularError {
    #[error("failed to read {path}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to open spreadsheet {path}")]
    Spreadsheet {
        path: String,
        #[source]
        source: calamine::Error,
    },
    #[error("malformed delimited data in {path}")]
    Delimited {
        path: String,
        #[source]
        source: csv::Error,
    },
    #[error("{path} contains no tabular data")]
    Empty { path: String },
}

/// A single normalised table (one spreadsheet sheet or one CSV/TSV file).
#[derive(Debug, Clone, PartialEq)]
pub struct ExtractedTable {
    /// Sheet name for spreadsheets, file name for delimited files.
    pub name: String,
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

/// Delimited-text extensions (`.tsv` is always tab-separated; the `.csv`
/// delimiter is sniffed from content, see [`sniff_csv_delimiter`]).
pub fn is_delimited_extension(extension: &str) -> bool {
    matches!(extension, "csv" | "tsv")
}

/// Spreadsheet extensions readable through calamine.
pub fn is_spreadsheet_extension(extension: &str) -> bool {
    matches!(extension, "xlsx" | "xls" | "xlsm" | "xlsb" | "ods")
}

/// Normalise a raw cell grid into `(headers, data_rows)`.
///
/// - Cells are trimmed.
/// - Fully-empty rows are dropped (including leading empty rows, so the first
///   non-empty row becomes the header row).
/// - Columns that are empty in every remaining row are dropped.
/// - Ragged rows are padded so every row has one value per header.
/// - Blank header cells are named `Column N` (1-based, after column removal).
///
/// Returns `None` when the grid has no non-empty cell.
pub fn normalize_grid(grid: Vec<Vec<String>>) -> Option<(Vec<String>, Vec<Vec<String>>)> {
    let rows: Vec<Vec<String>> = grid
        .into_iter()
        .map(|row| row.into_iter().map(|c| c.trim().to_string()).collect())
        .filter(|row: &Vec<String>| row.iter().any(|c| !c.is_empty()))
        .collect();

    let width = rows.iter().map(Vec::len).max()?;
    let kept_columns: Vec<usize> = (0..width)
        .filter(|&col| {
            rows.iter()
                .any(|row| row.get(col).is_some_and(|v| !v.is_empty()))
        })
        .collect();

    let mut projected = rows.into_iter().map(|row| {
        kept_columns
            .iter()
            .map(|&col| row.get(col).cloned().unwrap_or_default())
            .collect::<Vec<String>>()
    });

    let mut headers = projected.next()?;
    for (idx, header) in headers.iter_mut().enumerate() {
        if header.is_empty() {
            *header = format!("Column {}", idx + 1);
        }
    }

    Some((headers, projected.collect()))
}

/// Render a float the way a spreadsheet displays it: whole numbers without a
/// fractional part (`1200.0` → `1200`) and fractional numbers rounded to 15
/// significant digits (Excel's precision) so binary noise such as
/// `0.30000000000000004` renders as `0.3`.
pub fn format_float(value: f64) -> String {
    if !value.is_finite() {
        return value.to_string();
    }
    if value == value.trunc() && value.abs() < 1e15 {
        // Exact: |value| < 1e15 fits in i64 and has no fractional part.
        // `as i64` also normalises -0.0 to 0.
        return (value as i64).to_string();
    }
    let rounded = format!("{:.14e}", value).parse::<f64>().unwrap_or(value);
    rounded.to_string()
}

/// Render an Excel date/time serial.
///
/// - Date-only values (midnight) → `yyyy-mm-dd`
/// - Time-only values (serial in `[0, 1)`) → `HH:MM:SS`
/// - Date + time → `yyyy-mm-dd HH:MM:SS`
/// - Durations (`[h]:mm:ss` formats) → `H:MM:SS` with unbounded hours
pub fn format_excel_datetime(dt: &ExcelDateTime) -> String {
    let serial = dt.as_f64();
    if dt.is_duration() {
        return format_duration_days(serial);
    }
    if (0.0..1.0).contains(&serial) {
        return format_duration_days(serial);
    }
    match dt.as_datetime() {
        Some(ndt) if ndt.time().num_seconds_from_midnight() == 0 && ndt.nanosecond() == 0 => {
            ndt.format("%Y-%m-%d").to_string()
        }
        Some(ndt) => ndt.format("%Y-%m-%d %H:%M:%S").to_string(),
        None => format_float(serial),
    }
}

/// Format a fraction-of-days value as `H:MM:SS` (hours may exceed 24).
fn format_duration_days(days: f64) -> String {
    if !days.is_finite() {
        return days.to_string();
    }
    let total_seconds = (days * 86_400.0).round();
    let sign = if total_seconds < 0.0 { "-" } else { "" };
    let total_seconds = total_seconds.abs() as u64;
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;
    format!("{}{:02}:{:02}:{:02}", sign, hours, minutes, seconds)
}

/// Convert a calamine cell into display text.
pub fn format_cell(cell: &Data) -> String {
    match cell {
        Data::Empty => String::new(),
        Data::String(s) => s.clone(),
        Data::Int(i) => i.to_string(),
        Data::Float(f) => format_float(*f),
        Data::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
        Data::Error(e) => e.to_string(),
        Data::DateTime(dt) => format_excel_datetime(dt),
        Data::DateTimeIso(s) => s.clone(),
        Data::DurationIso(s) => s.clone(),
    }
}

/// Decode raw bytes into text.
///
/// Order matters: a byte-order mark is authoritative (so UTF-16 is never
/// misread as a single-byte encoding), then valid UTF-8 is taken as-is, and
/// only then is the encoding guessed with chardetng (e.g. Windows-1252 CSVs
/// exported by Excel).
pub fn decode_text(bytes: &[u8]) -> (String, &'static Encoding) {
    if let Some((encoding, bom_len)) = Encoding::for_bom(bytes) {
        let (text, _had_errors) = encoding.decode_without_bom_handling(&bytes[bom_len..]);
        return (text.into_owned(), encoding);
    }
    if let Ok(text) = std::str::from_utf8(bytes) {
        return (text.to_owned(), encoding_rs::UTF_8);
    }
    let mut detector = chardetng::EncodingDetector::new();
    detector.feed(bytes, true);
    let encoding = detector.guess(None, true);
    let (text, _had_errors) = encoding.decode_without_bom_handling(bytes);
    (text.into_owned(), encoding)
}

/// Read a text file of unknown encoding and decode it to a `String`.
pub fn read_text_file(path: &Path) -> Result<String, TabularError> {
    let bytes = std::fs::read(path).map_err(|source| TabularError::Io {
        path: path.display().to_string(),
        source,
    })?;
    let (text, encoding) = decode_text(&bytes);
    if encoding != encoding_rs::UTF_8 {
        tracing::debug!(
            encoding = encoding.name(),
            "Decoded {} from non-UTF-8 encoding",
            path.display()
        );
    }
    Ok(text)
}

/// Pick the delimiter for a `.csv` file from its first non-empty line.
/// Semicolon-separated "CSV" (common in locales that use a decimal comma) and
/// tab-separated content saved as `.csv` are detected; comma is the default.
pub fn sniff_csv_delimiter(text: &str) -> u8 {
    let first_line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let mut in_quotes = false;
    let (mut commas, mut semicolons, mut tabs) = (0usize, 0usize, 0usize);
    for ch in first_line.chars() {
        match ch {
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => commas += 1,
            ';' if !in_quotes => semicolons += 1,
            '\t' if !in_quotes => tabs += 1,
            _ => {}
        }
    }
    if tabs > commas && tabs >= semicolons {
        b'\t'
    } else if semicolons > commas {
        b';'
    } else {
        b','
    }
}

/// Parse delimited text into a raw grid. Records may have differing lengths;
/// quoted fields (including embedded delimiters and newlines) are honoured.
pub fn parse_delimited(text: &str, delimiter: u8) -> Result<Vec<Vec<String>>, csv::Error> {
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .has_headers(false)
        .flexible(true)
        .from_reader(text.as_bytes());

    let mut grid = Vec::new();
    for record in reader.records() {
        let record = record?;
        grid.push(record.iter().map(str::to_string).collect());
    }
    Ok(grid)
}

/// Extract the single table contained in a CSV or TSV file.
pub fn read_delimited_file(path: &Path, extension: &str) -> Result<ExtractedTable, TabularError> {
    let path_str = path.display().to_string();
    let text = read_text_file(path)?;
    let delimiter = if extension == "tsv" {
        b'\t'
    } else {
        sniff_csv_delimiter(&text)
    };
    let grid = parse_delimited(&text, delimiter).map_err(|source| TabularError::Delimited {
        path: path_str.clone(),
        source,
    })?;
    let (headers, rows) = normalize_grid(grid).ok_or(TabularError::Empty { path: path_str })?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("Table")
        .to_string();
    Ok(ExtractedTable {
        name,
        headers,
        rows,
    })
}

/// Extract one table per non-empty sheet of a workbook.
/// Sheets that cannot be read are logged and skipped; an error is returned
/// only when the workbook cannot be opened or contains no data at all.
pub fn read_spreadsheet(path: &Path) -> Result<Vec<ExtractedTable>, TabularError> {
    let path_str = path.display().to_string();
    let mut workbook = open_workbook_auto(path).map_err(|source| TabularError::Spreadsheet {
        path: path_str.clone(),
        source,
    })?;

    let sheet_names: Vec<String> = workbook.sheet_names().to_vec();
    let mut tables = Vec::with_capacity(sheet_names.len());

    for sheet_name in sheet_names {
        let range = match workbook.worksheet_range(&sheet_name) {
            Ok(range) => range,
            Err(e) => {
                tracing::warn!(
                    sheet = %sheet_name,
                    error = %e,
                    "Skipping unreadable sheet in {}",
                    path_str
                );
                continue;
            }
        };
        let grid: Vec<Vec<String>> = range
            .rows()
            .map(|row| row.iter().map(format_cell).collect())
            .collect();
        if let Some((headers, rows)) = normalize_grid(grid) {
            tables.push(ExtractedTable {
                name: sheet_name,
                headers,
                rows,
            });
        }
    }

    if tables.is_empty() {
        return Err(TabularError::Empty { path: path_str });
    }
    Ok(tables)
}

/// Flat text rendering of tables (one row per line, pipe-separated), used as
/// the document's plain `content`.
pub fn tables_to_text(tables: &[ExtractedTable]) -> String {
    let mut out = String::new();
    for table in tables {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str("--- ");
        out.push_str(&table.name);
        out.push_str(" ---\n");
        out.push_str(&table.headers.join(" | "));
        out.push('\n');
        for row in &table.rows {
            out.push_str(&row.join(" | "));
            out.push('\n');
        }
    }
    out
}

/// Column headers whose non-empty values are mostly numeric (used downstream
/// for chart generation hints).
pub fn numeric_columns(table: &ExtractedTable) -> Vec<&str> {
    table
        .headers
        .iter()
        .enumerate()
        .filter(|(col, _)| {
            let mut non_empty = 0usize;
            let mut numeric = 0usize;
            for row in &table.rows {
                if let Some(value) = row.get(*col).filter(|v| !v.is_empty()) {
                    non_empty += 1;
                    if value.parse::<f64>().is_ok() {
                        numeric += 1;
                    }
                }
            }
            numeric > 0 && numeric * 2 >= non_empty
        })
        .map(|(_, header)| header.as_str())
        .collect()
}

/// Convert tables to chunker sections. Tables from spreadsheets / CSV files
/// have no page, so `page` is 0 ("not paged") and the table name is the caption.
pub fn tables_to_sections(tables: Vec<ExtractedTable>) -> Vec<DocumentSection> {
    tables
        .into_iter()
        .map(|t| DocumentSection::Table {
            headers: t.headers,
            rows: t.rows,
            page: 0,
            caption: Some(t.name),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use calamine::ExcelDateTimeType;

    fn grid(rows: &[&[&str]]) -> Vec<Vec<String>> {
        rows.iter()
            .map(|r| r.iter().map(|c| c.to_string()).collect())
            .collect()
    }

    #[test]
    fn windows_1252_csv_with_quoted_commas_decodes_and_parses() {
        // "Café" (0xE9), curly apostrophe (0x92), euro sign (0x80), "Müller" (0xFC)
        let mut bytes: Vec<u8> = Vec::new();
        bytes.extend_from_slice(b"Name,Note,Amount\r\n");
        bytes.extend_from_slice(
            b"\"Caf\xE9 M\xFCller\",\"It\x92s cheap, really\",\"\x80 1,200\"\r\n",
        );
        bytes.extend_from_slice(b"\"Stra\xDFe\",\"na\xEFve, d\xE9j\xE0 vu\",42\r\n");
        bytes.extend_from_slice(b"Jos\xE9,\"Fran\xE7ais, cr\xE8me br\xFBl\xE9e\",7\r\n");

        let (text, encoding) = decode_text(&bytes);
        assert_ne!(encoding, encoding_rs::UTF_8);
        assert!(text.contains("Café Müller"));
        assert!(text.contains("It\u{2019}s cheap, really"));
        assert!(text.contains("\u{20AC} 1,200"));

        assert_eq!(sniff_csv_delimiter(&text), b',');
        let parsed = parse_delimited(&text, b',').expect("valid csv");
        let (headers, rows) = normalize_grid(parsed).expect("non-empty");
        assert_eq!(headers, vec!["Name", "Note", "Amount"]);
        assert_eq!(rows.len(), 3);
        assert_eq!(
            rows[0],
            vec!["Café Müller", "It\u{2019}s cheap, really", "\u{20AC} 1,200"]
        );
        assert_eq!(rows[1], vec!["Straße", "naïve, déjà vu", "42"]);
        assert_eq!(rows[2], vec!["José", "Français, crème brûlée", "7"]);
    }

    #[test]
    fn utf8_bom_and_utf16_bom_are_decoded() {
        let mut utf8 = vec![0xEF, 0xBB, 0xBF];
        utf8.extend_from_slice("a,b\n1,ü\n".as_bytes());
        let (text, enc) = decode_text(&utf8);
        assert_eq!(enc, encoding_rs::UTF_8);
        assert_eq!(text, "a,b\n1,ü\n");

        let mut utf16le = vec![0xFF, 0xFE];
        for unit in "x\ty\n".encode_utf16() {
            utf16le.extend_from_slice(&unit.to_le_bytes());
        }
        let (text, enc) = decode_text(&utf16le);
        assert_eq!(enc, encoding_rs::UTF_16LE);
        assert_eq!(text, "x\ty\n");
    }

    #[test]
    fn tsv_and_flexible_records_parse() {
        let text = "a\tb\tc\n1\t2\n3\t4\t5\t6\n";
        let parsed = parse_delimited(text, b'\t').expect("valid tsv");
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[1], vec!["1", "2"]);
        assert_eq!(parsed[2].len(), 4);
    }

    #[test]
    fn sniffs_semicolon_and_tab_delimiters() {
        assert_eq!(sniff_csv_delimiter("a;b;c\n1,5;2;3"), b';');
        assert_eq!(sniff_csv_delimiter("a\tb\tc\n"), b'\t');
        assert_eq!(sniff_csv_delimiter("\"x;y\",b\n"), b',');
    }

    #[test]
    fn header_detection_skips_leading_empty_rows_and_columns() {
        let raw = grid(&[
            &["", "", ""],
            &["  ", "", ""],
            &["", "Region", "Sales", ""],
            &["", "North", "1200", ""],
            &["", "", "", ""],
            &["", "South", "", ""],
        ]);
        let (headers, rows) = normalize_grid(raw).expect("non-empty");
        assert_eq!(headers, vec!["Region", "Sales"]);
        assert_eq!(
            rows,
            vec![
                vec!["North".to_string(), "1200".to_string()],
                vec!["South".to_string(), String::new()],
            ]
        );
    }

    #[test]
    fn blank_header_cells_get_column_names_and_empty_grid_is_none() {
        let (headers, rows) = normalize_grid(grid(&[&["Id", ""], &["1", "x"]])).expect("non-empty");
        assert_eq!(headers, vec!["Id", "Column 2"]);
        assert_eq!(rows, vec![vec!["1".to_string(), "x".to_string()]]);

        assert!(normalize_grid(grid(&[&["", " "], &[]])).is_none());
        assert!(normalize_grid(Vec::new()).is_none());
    }

    #[test]
    fn numbers_render_without_float_noise() {
        assert_eq!(format_float(1200.0), "1200");
        assert_eq!(format_float(-0.0), "0");
        assert_eq!(format_float(-42.0), "-42");
        assert_eq!(format_float(0.1 + 0.2), "0.3");
        assert_eq!(format_float(1234.5678), "1234.5678");
        assert_eq!(format_float(19.99), "19.99");
        assert_eq!(format_cell(&Data::Float(3.0)), "3");
        assert_eq!(format_cell(&Data::Int(7)), "7");
        assert_eq!(format_cell(&Data::Bool(true)), "TRUE");
        assert_eq!(format_cell(&Data::Empty), "");
    }

    #[test]
    fn dates_render_as_iso() {
        // Serial 45292 = 2024-01-01 in the 1900 date system.
        let date = ExcelDateTime::new(45292.0, ExcelDateTimeType::DateTime, false);
        assert_eq!(format_cell(&Data::DateTime(date)), "2024-01-01");

        let date_time = ExcelDateTime::new(45292.5, ExcelDateTimeType::DateTime, false);
        assert_eq!(format_excel_datetime(&date_time), "2024-01-01 12:00:00");

        let time_only = ExcelDateTime::new(0.75, ExcelDateTimeType::DateTime, false);
        assert_eq!(format_excel_datetime(&time_only), "18:00:00");

        let duration = ExcelDateTime::new(1.5, ExcelDateTimeType::TimeDelta, false);
        assert_eq!(format_excel_datetime(&duration), "36:00:00");

        assert_eq!(
            format_cell(&Data::DateTimeIso("2024-02-29".to_string())),
            "2024-02-29"
        );
    }

    #[test]
    fn tables_become_unpaged_sections_with_name_caption() {
        let table = ExtractedTable {
            name: "Q1".to_string(),
            headers: vec!["Item".to_string(), "Qty".to_string()],
            rows: vec![vec!["Bolt".to_string(), "10".to_string()]],
        };
        assert_eq!(numeric_columns(&table), vec!["Qty"]);
        let sections = tables_to_sections(vec![table]);
        match &sections[0] {
            DocumentSection::Table { page, caption, .. } => {
                assert_eq!(*page, 0);
                assert_eq!(caption.as_deref(), Some("Q1"));
            }
            other => panic!("expected table section, got {:?}", other),
        }
    }

    #[test]
    fn extension_helpers_cover_new_types() {
        for ext in ["xlsx", "xls", "xlsm", "xlsb", "ods"] {
            assert!(is_spreadsheet_extension(ext), "{}", ext);
        }
        assert!(is_delimited_extension("csv"));
        assert!(is_delimited_extension("tsv"));
        assert!(!is_delimited_extension("txt"));
    }
}
