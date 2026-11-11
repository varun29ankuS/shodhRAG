//! Tests of [`super::formats`], with the fixtures the parser's tests use too.

/// Files of the formats [`super::formats`] reads, built in memory.
pub(crate) mod fixtures {
    use std::io::{Cursor, Write};

    /// A compound file (version 3, 512-byte sectors, as Office writes) holding
    /// `streams`.
    pub fn compound_file(streams: &[(&str, &[u8])]) -> Vec<u8> {
        let mut file =
            cfb::CompoundFile::create_with_version(cfb::Version::V3, Cursor::new(Vec::new()))
                .unwrap();
        for (name, data) in streams {
            let mut stream = file.create_stream(format!("/{name}")).unwrap();
            stream.write_all(data).unwrap();
        }
        file.flush().unwrap();
        file.into_inner().into_inner()
    }

    /// A Word 97 file whose main text is `text` (Windows-1252, one piece),
    /// marked encrypted when `encrypted`.
    pub fn word_doc(text: &str, encrypted: bool) -> Vec<u8> {
        let (bytes, _, _) = encoding_rs::WINDOWS_1252.encode(text);
        let mut fib = vec![0u8; 1024];
        fib[0..2].copy_from_slice(&0xA5ECu16.to_le_bytes());
        fib[2..4].copy_from_slice(&0x00C1u16.to_le_bytes());
        // fWhichTblStm (the 1Table stream), and fEncrypted when asked.
        let flags: u16 = (1 << 9) | if encrypted { 1 << 8 } else { 0 };
        fib[0x0A..0x0C].copy_from_slice(&flags.to_le_bytes());
        fib[0x4C..0x50].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
        // One piece: CPs 0 and n, then its PCD (compressed text at byte 1024).
        let mut clx = vec![0x02];
        clx.extend_from_slice(&16u32.to_le_bytes());
        clx.extend_from_slice(&0u32.to_le_bytes());
        clx.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        clx.extend_from_slice(&0u16.to_le_bytes());
        clx.extend_from_slice(&(0x4000_0000u32 | (1024 * 2)).to_le_bytes());
        clx.extend_from_slice(&0u16.to_le_bytes());
        fib[0x01A2..0x01A6].copy_from_slice(&0u32.to_le_bytes());
        fib[0x01A6..0x01AA].copy_from_slice(&(clx.len() as u32).to_le_bytes());
        let mut word = fib;
        word.extend_from_slice(&bytes);
        compound_file(&[("WordDocument", &word), ("1Table", &clx)])
    }

    /// A PowerPoint record: its header (version, type, length) and `data`.
    fn record(container: bool, rec_type: u16, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(8 + data.len());
        let version: u16 = if container { 0x000F } else { 0 };
        out.extend_from_slice(&version.to_le_bytes());
        out.extend_from_slice(&rec_type.to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
        out
    }

    /// A PowerPoint 97 file with one slide per entry of `slides` (its body text,
    /// as UTF-16), and an `EncryptedSummary` stream when `encrypted`.
    pub fn power_point(slides: &[&str], encrypted: bool) -> Vec<u8> {
        let mut document = Vec::new();
        for text in slides {
            let mut body = record(false, 0x0F9F, &1u32.to_le_bytes());
            let chars: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
            body.extend(record(false, 0x0FA0, &chars));
            document.extend(record(true, 0x03EE, &body));
        }
        let mut streams: Vec<(&str, &[u8])> = vec![("PowerPoint Document", &document)];
        if encrypted {
            streams.push(("EncryptedSummary", b"\x00\x00\x00\x00"));
        }
        compound_file(&streams)
    }

    /// A zip archive holding `entries` (name, content).
    pub fn zip(entries: &[(&str, &str)]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        for (name, content) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(content.as_bytes()).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }
}

use super::formats::*;

#[test]
fn rtf_text_keeps_the_words_and_drops_the_markup() {
    let rtf = concat!(
        r"{\rtf1\ansi\ansicpg1252\deff0{\fonttbl{\f0\fswiss Helvetica;}}{\colortbl;\red255\green0\blue0;}",
        "\n",
        r"{\info{\title Secret title}{\author A}}{\*\generator Msftedit 5.41;}",
        "\n",
        r"\pard\b Notice period\b0\par",
        "\n",
        r"The lease ends after ninety days\'92 notice.\tab Rent: \u8364?120.\par",
        "\n",
        r#"{\*\bkmkstart one}{\field{\*\fldinst HYPERLINK "http://x"}{\fldrslt Read the terms}}\par"#,
        "\n",
        r"{\pict\pngblip 89504e470d0a}Caf\'e9 \{braces\} back\\slash\line next}",
    );
    let text = rtf_text(rtf.as_bytes()).unwrap();
    assert!(text.contains("Notice period"), "{text}");
    assert!(
        text.contains("The lease ends after ninety days\u{2019} notice.\tRent: \u{20ac}120."),
        "{text}"
    );
    assert!(text.contains("Read the terms"), "{text}");
    assert!(
        text.contains("Caf\u{e9} {braces} back\\slash\nnext"),
        "{text}"
    );
    for markup in [
        "Helvetica",
        "Secret title",
        "Msftedit",
        "HYPERLINK",
        "89504e",
        "fonttbl",
        "\\b",
    ] {
        assert!(!text.contains(markup), "{markup} in {text}");
    }
}

#[test]
fn hostile_rtf_fails_plainly() {
    assert!(matches!(rtf_text(b"plain text"), Err(RtfError::NotRtf)));
    let deep = format!("{{\\rtf1 {}", "{".repeat(MAX_RTF_DEPTH + 1));
    assert!(matches!(rtf_text(deep.as_bytes()), Err(RtfError::TooDeep)));
    // Unclosed groups and a truncated escape still read what is there.
    assert_eq!(
        rtf_text(b"{\\rtf1 Hello {\\b world\\'9").unwrap(),
        "Hello world"
    );
    // \bin data is skipped whole.
    assert_eq!(rtf_text(b"{\\rtf1 a\\bin3 xyzb}").unwrap(), "ab");
}

#[test]
fn an_email_reads_as_headers_body_and_attachment_names() {
    let eml = [
        "From: =?utf-8?B?QXNoYSBSYW8=?= <asha@example.org>",
        "To: team@example.org",
        "Subject: =?iso-8859-1?Q?Caf=E9_lease?= renewal",
        "Date: Mon, 3 Nov 2026 10:00:00 +0530",
        "MIME-Version: 1.0",
        "Content-Type: multipart/mixed; boundary=\"outer\"",
        "",
        "preamble",
        "--outer",
        "Content-Type: multipart/alternative; boundary=\"inner\"",
        "",
        "--inner",
        "Content-Type: text/plain; charset=utf-8",
        "Content-Transfer-Encoding: quoted-printable",
        "",
        "The notice period is ninety days. Rent is =E2=82=AC120 a=",
        " month.",
        "--inner",
        "Content-Type: text/html; charset=utf-8",
        "",
        "<p>The notice period is <b>ninety</b> days.</p>",
        "--inner--",
        "--outer",
        "Content-Type: application/pdf; name=\"lease.pdf\"",
        "Content-Disposition: attachment; filename=\"lease.pdf\"",
        "Content-Transfer-Encoding: base64",
        "",
        "JVBERi0xLjQK",
        "--outer--",
        "",
    ]
    .join("\r\n");
    let email = eml_text(eml.as_bytes(), |html| {
        html.replace("<p>", "").replace("</p>", "")
    });
    assert_eq!(email.subject.as_deref(), Some("Caf\u{e9} lease renewal"));
    assert_eq!(email.attachments, vec!["lease.pdf".to_string()]);
    assert!(
        email
            .text
            .starts_with("From: Asha Rao <asha@example.org>\nTo: team@example.org\n"),
        "{}",
        email.text
    );
    assert!(
        email.text.contains("Subject: Caf\u{e9} lease renewal"),
        "{}",
        email.text
    );
    assert!(
        email.text.contains("Rent is \u{20ac}120 a month."),
        "{}",
        email.text
    );
    // The plain part is read; its HTML copy is not read again.
    assert_eq!(
        email.text.matches("notice period").count(),
        1,
        "{}",
        email.text
    );
    assert!(
        email.text.ends_with("Attachments (not indexed): lease.pdf"),
        "{}",
        email.text
    );
    assert!(!email.text.contains("JVBER"));
}

#[test]
fn an_html_only_email_is_read_as_text() {
    let eml = "Subject: Hi\nContent-Type: text/html\nContent-Transfer-Encoding: base64\n\nPGI+SGVsbG88L2I+IHRoZXJl\n";
    let email = eml_text(eml.as_bytes(), |html| {
        html.replace("<b>", "").replace("</b>", "")
    });
    assert!(email.text.ends_with("Hello there"), "{}", email.text);
}

#[test]
fn a_notebook_reads_cell_by_cell_with_its_text_outputs() {
    let long = "x".repeat(MAX_NOTEBOOK_OUTPUT_CHARS * 3);
    let notebook = serde_json::json!({
        "cells": [
            {"cell_type": "markdown", "source": ["# Rainfall\n", "Daily totals by station."]},
            {"cell_type": "code", "source": "df.mean()", "outputs": [
                {"output_type": "execute_result", "data": {"text/plain": ["station_a    12.5\n"], "image/png": "iVBOR"}},
                {"output_type": "stream", "text": long},
            ]},
            {"cell_type": "code", "source": "1/0", "outputs": [
                {"output_type": "error", "ename": "ZeroDivisionError", "evalue": "division by zero"}
            ]},
            {"cell_type": "code", "source": "", "outputs": []},
        ]
    });
    let text = notebook_text(&notebook.to_string()).unwrap();
    assert!(
        text.starts_with("--- Cell 1 (markdown) ---\n# Rainfall\nDaily totals by station."),
        "{text}"
    );
    assert!(
        text.contains("--- Cell 2 (code) ---\ndf.mean()\n\nOutput:\nstation_a    12.5"),
        "{text}"
    );
    assert!(
        text.contains("ZeroDivisionError: division by zero"),
        "{text}"
    );
    assert!(
        !text.contains("iVBOR") && !text.contains("Cell 4"),
        "{text}"
    );
    let cell2 = text.split("--- Cell 3").next().unwrap();
    assert!(
        cell2.matches('x').count() <= MAX_NOTEBOOK_OUTPUT_CHARS,
        "outputs are capped"
    );
    assert!(matches!(
        notebook_text("{\"cells\": [trunc"),
        Err(NotebookError::Json(_))
    ));
    assert!(matches!(
        notebook_text("{\"metadata\": {}}"),
        Err(NotebookError::NoCells)
    ));
}

#[test]
fn a_word_97_file_is_read() {
    let doc = fixtures::word_doc(
        "Lease terms\rThe notice period is ninety days.\rRent: 120\u{20ac} a month.\r",
        false,
    );
    let text = doc_text(&doc).unwrap();
    assert_eq!(
        text,
        "Lease terms\nThe notice period is ninety days.\nRent: 120\u{20ac} a month."
    );
}

#[test]
fn broken_and_protected_word_files_say_why() {
    let doc = fixtures::word_doc("Secret plans.\r", true);
    let err = doc_text(&doc).unwrap_err();
    assert!(matches!(err, LegacyOfficeError::Encrypted("Word")), "{err}");
    assert!(
        err.to_string().contains("protected with a password"),
        "{err}"
    );

    let whole = fixtures::word_doc("The notice period is ninety days.\r", false);
    for cut in [0, 100, 512, 700, whole.len() / 2] {
        let err = doc_text(&whole[..cut]).unwrap_err();
        assert!(
            err.to_string().contains("damaged or incomplete"),
            "{cut}: {err}"
        );
    }
    let err = doc_text(b"this is not a compound file at all").unwrap_err();
    assert!(err.to_string().contains("damaged or incomplete"), "{err}");

    // A compound file that is not a Word document.
    let other = fixtures::compound_file(&[("Workbook", b"\x09\x08")]);
    assert!(doc_text(&other)
        .unwrap_err()
        .to_string()
        .contains("damaged or incomplete"));

    // Word 6.0/95.
    let mut old = fixtures::word_doc("x\r", false);
    let marker = 0xA5ECu16.to_le_bytes();
    let at = old.windows(2).position(|w| w == marker).unwrap();
    old[at..at + 2].copy_from_slice(&0xA5DCu16.to_le_bytes());
    let err = doc_text(&old).unwrap_err();
    assert!(matches!(err, LegacyOfficeError::TooOld(..)), "{err}");
    assert!(err.to_string().contains("not supported"), "{err}");
}

/// A directory whose entries loop would send office_oxide's recursive search
/// round until the stack ran out: it is refused before it is searched.
#[test]
fn a_compound_file_whose_directory_loops_is_refused() {
    let mut doc = fixtures::word_doc("Loop.\r", false);
    let first_dir_sector = u32::from_le_bytes(doc[0x30..0x34].try_into().unwrap()) as usize;
    let dir = 512 + first_dir_sector * 512;
    // The root's child is entry 1, and entry 1 is its own left sibling.
    doc[dir + 0x4C..dir + 0x50].copy_from_slice(&1u32.to_le_bytes());
    doc[dir + 128 + 0x44..dir + 128 + 0x48].copy_from_slice(&1u32.to_le_bytes());
    let err = doc_text(&doc).unwrap_err();
    assert!(
        err.to_string()
            .contains("damaged or incomplete (its directory loops)"),
        "{err}"
    );
    assert!(ppt_slides(&doc).unwrap_err().to_string().contains("loops"));
}

#[test]
fn a_power_point_97_file_is_read_slide_by_slide() {
    let ppt = fixtures::power_point(&["Rainfall study", "", "Results\rNext steps"], false);
    let slides = ppt_slides(&ppt).unwrap();
    assert_eq!(slides, vec!["Rainfall study", "", "Results\nNext steps"]);

    let err = ppt_slides(&fixtures::power_point(&["x"], true)).unwrap_err();
    assert!(
        err.to_string().contains("protected with a password"),
        "{err}"
    );
    let err = ppt_slides(&fixtures::power_point(&[], false)).unwrap_err();
    assert!(
        err.to_string().contains("contains no extractable text"),
        "{err}"
    );
    let err = ppt_slides(&fixtures::compound_file(&[("WordDocument", b"x")])).unwrap_err();
    assert!(err.to_string().contains("damaged or incomplete"), "{err}");
    let whole = fixtures::power_point(&["Rainfall study"], false);
    assert!(ppt_slides(&whole[..whole.len() / 2]).is_err());
}

#[test]
fn epub_links_resolve_inside_the_book() {
    assert_eq!(
        resolve_href("OEBPS/content.opf", "text/ch%201.xhtml#top").as_deref(),
        Some("OEBPS/text/ch 1.xhtml")
    );
    assert_eq!(
        resolve_href("OEBPS/content.opf", "../ch2.xhtml").as_deref(),
        Some("ch2.xhtml")
    );
    assert_eq!(
        resolve_href("content.opf", "ch3.xhtml").as_deref(),
        Some("ch3.xhtml")
    );
    assert_eq!(resolve_href("content.opf", "../../etc/passwd"), None);
    let plain = |html: &str| html.replace("<em>", "").replace("</em>", "");
    assert_eq!(
        chapter_title(
            "<html><head><title>Book</title></head><body><h1 class=\"c\">The <em>Long</em> Rain</h1></body></html>",
            &plain
        )
        .as_deref(),
        Some("The Long Rain")
    );
    assert_eq!(
        chapter_title(
            "<html><head><title>Only title</title></head></html>",
            &plain
        )
        .as_deref(),
        Some("Only title")
    );
    assert_eq!(chapter_title("<p>none</p>", &plain), None);
}
