//! The largest files the indexer reads, by kind, in one place. A file over
//! its limit is refused before it is read ([`check_file_size`]), and a PDF
//! with more pages than [`MAX_PDF_PAGES`] before its pages are parsed
//! ([`check_pdf_pages`]): the whole file is held in memory several times
//! while it is parsed, chunked and embedded, and a file far larger than any
//! document would hold the indexer for hours. The limits are generous; a
//! refused file is reported as "too large to index (limit …)".

/// One mebibyte, the unit the app shows file sizes in ("MB").
pub const MIB: u64 = 1024 * 1024;

/// Plain text, Markdown, LaTeX, HTML, JSON, code and configuration files.
pub const MAX_TEXT_BYTES: u64 = 50 * MIB;

/// Spreadsheets and delimited files (CSV, TSV, XLSX, ODS, ...).
pub const MAX_TABLE_BYTES: u64 = 100 * MIB;

/// Word and PowerPoint files (compressed, as stored).
pub const MAX_OFFICE_BYTES: u64 = 100 * MIB;

/// One XML part of a Word or PowerPoint file once decompressed: the file's
/// size does not bound it.
pub const MAX_OFFICE_PART_BYTES: u64 = 256 * MIB;

/// Images read by OCR.
pub const MAX_IMAGE_BYTES: u64 = 100 * MIB;

/// PDFs are limited by pages ([`MAX_PDF_PAGES`]); this only keeps a file far
/// larger than any document from being read into memory to count them.
pub const MAX_PDF_BYTES: u64 = 1024 * MIB;

/// The most pages of a PDF that are indexed.
pub const MAX_PDF_PAGES: usize = 2_000;

/// A file over its limit: what it is (`This file is 63.2 MB`, `This PDF has
/// 2,431 pages`) and the limit (`50 MB for text files`).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{what}, too large to index (limit {limit})")]
pub struct TooLarge {
    pub what: String,
    pub limit: String,
}

/// The kinds of file the limits tell apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileKind {
    Text,
    Table,
    Office,
    Image,
    Pdf,
}

impl FileKind {
    fn of(extension: &str) -> Self {
        let extension = extension.to_ascii_lowercase();
        let extension = extension.as_str();
        if super::tabular::is_spreadsheet_extension(extension)
            || super::tabular::is_delimited_extension(extension)
        {
            return Self::Table;
        }
        match extension {
            "pdf" => Self::Pdf,
            "docx" | "pptx" => Self::Office,
            "png" | "jpg" | "jpeg" | "bmp" | "tiff" | "tif" => Self::Image,
            _ => Self::Text,
        }
    }

    fn byte_limit(self) -> (u64, &'static str) {
        match self {
            Self::Text => (MAX_TEXT_BYTES, "text files"),
            Self::Table => (MAX_TABLE_BYTES, "spreadsheets"),
            Self::Office => (MAX_OFFICE_BYTES, "Word and PowerPoint files"),
            Self::Image => (MAX_IMAGE_BYTES, "images"),
            Self::Pdf => (MAX_PDF_BYTES, "PDFs"),
        }
    }
}

/// The byte limit of files with `extension` (without the dot, any case).
pub fn byte_limit(extension: &str) -> u64 {
    FileKind::of(extension).byte_limit().0
}

/// Refuses a file of `size` bytes with `extension` when it is over its
/// kind's limit.
pub fn check_file_size(extension: &str, size: u64) -> Result<(), TooLarge> {
    let (limit, kind) = FileKind::of(extension).byte_limit();
    if size <= limit {
        return Ok(());
    }
    Err(TooLarge {
        what: format!("This file is {}", megabytes(size)),
        limit: format!("{} for {kind}", megabytes(limit)),
    })
}

/// Refuses a PDF of `pages` pages when it has more than [`MAX_PDF_PAGES`].
pub fn check_pdf_pages(pages: usize) -> Result<(), TooLarge> {
    if pages <= MAX_PDF_PAGES {
        return Ok(());
    }
    Err(TooLarge {
        what: format!("This PDF has {} pages", thousands(pages as u64)),
        limit: format!("{} pages", thousands(MAX_PDF_PAGES as u64)),
    })
}

/// Refuses an XML part of a Word or PowerPoint file (`name`) whose
/// decompressed size is over [`MAX_OFFICE_PART_BYTES`].
pub fn check_office_part(name: &str, size: u64) -> Result<(), TooLarge> {
    if size <= MAX_OFFICE_PART_BYTES {
        return Ok(());
    }
    Err(TooLarge {
        what: format!("Its part {name} is {} decompressed", megabytes(size)),
        limit: format!("{} for one part", megabytes(MAX_OFFICE_PART_BYTES)),
    })
}

/// `bytes` in MB, whole when exact ("50 MB"), else to one decimal ("63.2 MB").
fn megabytes(bytes: u64) -> String {
    if bytes % MIB == 0 {
        format!("{} MB", thousands(bytes / MIB))
    } else {
        format!("{:.1} MB", bytes as f64 / MIB as f64)
    }
}

/// `n` with thousands separated by commas ("2,431").
fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_kind_has_its_limit() {
        for (extension, limit) in [
            ("txt", MAX_TEXT_BYTES),
            ("md", MAX_TEXT_BYTES),
            ("rs", MAX_TEXT_BYTES),
            ("html", MAX_TEXT_BYTES),
            ("csv", MAX_TABLE_BYTES),
            ("XLSX", MAX_TABLE_BYTES),
            ("ods", MAX_TABLE_BYTES),
            ("docx", MAX_OFFICE_BYTES),
            ("pptx", MAX_OFFICE_BYTES),
            ("png", MAX_IMAGE_BYTES),
            ("tif", MAX_IMAGE_BYTES),
            ("pdf", MAX_PDF_BYTES),
            ("Pdf", MAX_PDF_BYTES),
        ] {
            assert_eq!(byte_limit(extension), limit, "{extension}");
            assert_eq!(check_file_size(extension, limit), Ok(()), "{extension}");
            assert!(
                check_file_size(extension, limit + 1).is_err(),
                "{extension}"
            );
        }
        assert_eq!(MAX_TEXT_BYTES, 50 * MIB);
        assert_eq!(MAX_TABLE_BYTES, 100 * MIB);
    }

    #[test]
    fn a_file_over_its_limit_says_how_large_it_is_and_the_limit() {
        let err = check_file_size("txt", MAX_TEXT_BYTES + 13 * MIB + MIB / 5).unwrap_err();
        assert_eq!(
            err.to_string(),
            "This file is 63.2 MB, too large to index (limit 50 MB for text files)"
        );
        let err = check_file_size("csv", 2 * MAX_TABLE_BYTES).unwrap_err();
        assert_eq!(
            err.to_string(),
            "This file is 200 MB, too large to index (limit 100 MB for spreadsheets)"
        );
        let err = check_office_part("word/document.xml", MAX_OFFICE_PART_BYTES + 1).unwrap_err();
        assert!(
            err.to_string()
                .starts_with("Its part word/document.xml is 256.0 MB decompressed, too large"),
            "{err}"
        );
    }

    #[test]
    fn pdfs_are_limited_by_pages() {
        assert_eq!(check_pdf_pages(0), Ok(()));
        assert_eq!(check_pdf_pages(MAX_PDF_PAGES), Ok(()));
        let err = check_pdf_pages(2_431).unwrap_err();
        assert_eq!(
            err.to_string(),
            "This PDF has 2,431 pages, too large to index (limit 2,000 pages)"
        );
    }

    #[test]
    fn thousands_are_separated() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_000), "1,000");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }
}
