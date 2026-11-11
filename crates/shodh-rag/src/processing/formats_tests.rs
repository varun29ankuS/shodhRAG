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

/// 8-bit text is read in the code page `\ansicpgN` names: Cyrillic (1251), Greek
/// (1253) and Shift-JIS (932, two escapes to a character), not always as 1252.
#[test]
fn rtf_text_is_decoded_in_its_code_page() {
    // "Привет" in Windows-1251.
    assert_eq!(
        rtf_text(br"{\rtf1\ansi\ansicpg1251 \'cf\'f0\'e8\'e2\'e5\'f2}").unwrap(),
        "Привет"
    );
    // "Αθήνα" in Windows-1253.
    assert_eq!(
        rtf_text(br"{\rtf1\ansi\ansicpg1253 \'c1\'e8\'de\'ed\'e1}").unwrap(),
        "Αθήνα"
    );
    // "あい" in Shift-JIS: each character two escapes.
    assert_eq!(
        rtf_text(br"{\rtf1\ansi\ansicpg932 \'82\'a0\'82\'a2}").unwrap(),
        "あい"
    );
    // Raw 8-bit bytes too, and no code page is 1252.
    assert_eq!(
        rtf_text(b"{\\rtf1\\ansi\\ansicpg1251 \xcf\xf0}").unwrap(),
        "Пр"
    );
    assert_eq!(rtf_text(br"{\rtf1\ansi caf\'e9}").unwrap(), "café");
}

/// A character outside the Basic Multilingual Plane is a `\u` surrogate pair:
/// read as one character, each half's fallback dropped. A lone half is U+FFFD.
#[test]
fn rtf_surrogate_pairs_are_one_character() {
    assert_eq!(
        rtf_text(br"{\rtf1\ansi\uc1 Rain \u-10180?\u-8409? today}").unwrap(),
        "Rain \u{1F327} today"
    );
    assert_eq!(rtf_text(br"{\rtf1 a\u-10180?b}").unwrap(), "a\u{FFFD}b");
}

/// Hidden text (`\v`) is not the document's text: it ends with `\v0`, `\plain`
/// or its group.
#[test]
fn rtf_hidden_text_is_left_out() {
    assert_eq!(
        rtf_text(br"{\rtf1 Shown {\v secret note} again \v hidden\v0  and \v gone\plain  back}")
            .unwrap(),
        "Shown  again  and  back"
    );
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

/// Attachments are listed by name and never read: a forwarded message attached
/// as a file, a text file sent inline with a name, a CSV with no disposition.
#[test]
fn an_emails_attachments_are_listed_not_read() {
    let eml = [
        "Subject: Files",
        "Content-Type: multipart/mixed; boundary=\"b\"",
        "",
        "--b",
        "Content-Type: text/plain",
        "",
        "See the attached files.",
        "--b",
        "Content-Type: message/rfc822; name=\"fwd.eml\"",
        "Content-Disposition: attachment; filename=\"fwd.eml\"",
        "",
        "Subject: inner",
        "",
        "INNER-BODY-TEXT",
        "--b",
        "Content-Type: text/plain; name=\"notes.txt\"",
        "Content-Disposition: inline; filename=\"notes.txt\"",
        "",
        "NAMED-INLINE-TEXT",
        "--b",
        "Content-Type: text/csv",
        "",
        "CSV-CELL,1",
        "--b",
        "Content-Type: text/calendar; method=REQUEST",
        "",
        "BEGIN:VCALENDAR",
        "--b--",
        "",
    ]
    .join("\r\n");
    let email = eml_text(eml.as_bytes(), |html| html.to_string());
    assert!(
        email.text.contains("See the attached files."),
        "{}",
        email.text
    );
    for unread in [
        "INNER-BODY-TEXT",
        "NAMED-INLINE-TEXT",
        "CSV-CELL",
        "VCALENDAR",
    ] {
        assert!(
            !email.text.contains(unread),
            "{unread} was read:\n{}",
            email.text
        );
    }
    assert_eq!(
        email.attachments,
        vec![
            "fwd.eml".to_string(),
            "notes.txt".to_string(),
            "an attachment (text/csv)".to_string(),
            "an attachment (text/calendar)".to_string(),
        ]
    );
}

/// Hostile structure: an empty boundary, deep nesting and many parts end the walk.
#[test]
fn hostile_email_structure_is_bounded() {
    let empty = "Content-Type: multipart/mixed; boundary=\"\"\n\n--\n--\nbody\n";
    assert!(eml_text(empty.as_bytes(), |h| h.to_string())
        .text
        .is_empty());
    // A message forwarded inline inside itself, deeper than the walk goes.
    let mut nested = String::from("Subject: top\nContent-Type: text/plain\n\nTOP-TEXT\n");
    for _ in 0..(MAX_MIME_DEPTH + 4) {
        nested = format!("Content-Type: message/rfc822\n\n{nested}");
    }
    let email = eml_text(nested.as_bytes(), |h| h.to_string());
    assert!(!email.text.contains("TOP-TEXT"), "{}", email.text);
    // More parts than the walk reads.
    let mut many = String::from("Content-Type: multipart/mixed; boundary=x\n\n");
    for i in 0..(MAX_MIME_PARTS + 50) {
        many.push_str(&format!("--x\nContent-Type: text/plain\n\npart-{i}\n"));
    }
    many.push_str("--x--\n");
    let email = eml_text(many.as_bytes(), |h| h.to_string());
    assert!(email.text.contains("part-0"));
    assert!(!email
        .text
        .contains(&format!("part-{}", MAX_MIME_PARTS + 10)));
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

/// What a notebook holds besides its text (metadata, widget state, images) is
/// skipped while it is parsed, however deeply it nests: a notebook is never
/// refused, nor built in memory, for parts that are not indexed. Odd values where
/// text is expected are no text.
#[test]
fn a_notebooks_other_parts_are_skipped_however_deep() {
    let depth = 5_000;
    let deep = format!("{}1{}", "[".repeat(depth), "]".repeat(depth));
    let json = format!(
        r#"{{"metadata": {{"widgets": {deep}}}, "nbformat": 4, "cells": [
            {{"cell_type": "markdown", "source": ["Intro ", {deep}, "text"], "attachments": {{"a.png": {deep}}}}},
            {{"cell_type": "code", "source": 42, "outputs": [
                {{"output_type": "display_data", "data": {{"image/png": {deep}, "text/plain": "plot shown"}}}},
                {{"output_type": "stream", "text": null}}
            ]}}
        ]}}"#
    );
    let text = notebook_text(&json).unwrap();
    assert!(
        text.contains("--- Cell 1 (markdown) ---\nIntro text"),
        "{text}"
    );
    assert!(text.contains("Output:\nplot shown"), "{text}");
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
    assert!(
        err.to_string().contains("saved in Word 95 or older"),
        "{err}"
    );
}

// ── The compound-file guard, on in-memory tables ──────────────────────────
//
// office_oxide follows these chains with no cycle check of its own; the guard
// walks them first. Its walks are pure functions over the tables a file holds,
// tested here on small synthetic tables.

const END: u32 = 0xFFFF_FFFE;
const FREE: u32 = 0xFFFF_FFFF;

fn damaged(result: Result<impl std::fmt::Debug, CfbIssue>) -> String {
    match result {
        Err(CfbIssue::Damaged(what)) => what,
        other => panic!("expected a damaged file, got {other:?}"),
    }
}

#[test]
fn a_valid_chain_is_walked_once_in_order() {
    // 0 → 2 → 1 → end; 3 is another chain.
    let fat = [2, END, 1, END];
    let mut owned = vec![false; 4];
    assert_eq!(
        walk_chain(&fat, 0, 4, 3, &mut owned).unwrap(),
        vec![0, 2, 1]
    );
    assert_eq!(walk_chain(&fat, 3, 4, 1, &mut owned).unwrap(), vec![3]);
    // An empty stream's chain starts at the end marker.
    assert!(walk_chain(&fat, END, 4, 0, &mut owned).unwrap().is_empty());
}

#[test]
fn a_chain_that_loops_is_refused() {
    // A sector that points to itself.
    let mut owned = vec![false; 2];
    assert!(damaged(walk_chain(&[0, END], 0, 2, 100, &mut owned)).contains("loops"));
    // A two-sector cycle.
    let mut owned = vec![false; 2];
    assert!(damaged(walk_chain(&[1, 0], 0, 2, 100, &mut owned)).contains("loops"));
    // Two chains sharing a sector.
    let fat = [2, 2, END];
    let mut owned = vec![false; 3];
    walk_chain(&fat, 0, 3, 2, &mut owned).unwrap();
    assert!(damaged(walk_chain(&fat, 1, 3, 2, &mut owned)).contains("loops"));
}

#[test]
fn a_chain_past_the_end_or_longer_than_its_size_is_refused() {
    let mut owned = vec![false; 3];
    assert!(damaged(walk_chain(&[1, 9, END], 0, 3, 10, &mut owned)).contains("past its end"));
    // The FAT may be longer than the file has sectors: those are past the end too.
    let mut owned = vec![false; 2];
    assert!(damaged(walk_chain(&[1, 2, END], 0, 2, 10, &mut owned)).contains("past its end"));
    // Three sectors for a stream whose size needs two.
    let mut owned = vec![false; 3];
    assert!(damaged(walk_chain(&[1, 2, END], 0, 3, 2, &mut owned)).contains("longer than its size"));
}

#[test]
fn a_stated_size_the_file_cannot_hold_is_refused() {
    assert!(check_stream_size(512 * 4, 4, 512).is_ok());
    assert!(damaged(check_stream_size(512 * 4 + 1, 4, 512)).contains("larger than the file"));
    assert!(check_stream_size(u64::MAX, 4, 512).is_err());
    assert_eq!(sectors_for(0, 512), 0);
    assert_eq!(sectors_for(1, 512), 1);
    assert_eq!(sectors_for(1024, 512), 2);
    assert_eq!(sectors_for(1025, 64), 17);
}

#[test]
fn the_fat_sector_list_is_walked_once() {
    let header: Vec<u32> = [1, 2]
        .into_iter()
        .chain(std::iter::repeat_n(FREE, 107))
        .collect();
    // No DIFAT chain: the header's two FAT sectors.
    let none = |_: u32| -> Option<Vec<u32>> { None };
    let mut owned = vec![false; 8];
    assert_eq!(
        fat_sector_ids(&header, END, &none, 8, &mut owned).unwrap(),
        vec![1, 2]
    );
    // A DIFAT sector (4) listing one more FAT sector (5), then the end.
    let difat = |id: u32| -> Option<Vec<u32>> { (id == 4).then(|| vec![5, FREE, END]) };
    let mut owned = vec![false; 8];
    assert_eq!(
        fat_sector_ids(&header, 4, &difat, 8, &mut owned).unwrap(),
        vec![1, 2, 5]
    );
}

#[test]
fn a_fat_sector_list_that_loops_or_repeats_is_refused() {
    let header = vec![FREE; 109];

    // A DIFAT sector whose next sector is itself.
    let self_loop = |id: u32| -> Option<Vec<u32>> { (id == 3).then(|| vec![FREE, 3]) };
    let mut owned = vec![false; 8];
    assert!(damaged(fat_sector_ids(&header, 3, &self_loop, 8, &mut owned)).contains("loops"));

    // Two DIFAT sectors pointing at each other: 3 → 4 → 3.
    let cycle = |id: u32| -> Option<Vec<u32>> {
        match id {
            3 => Some(vec![FREE, 4]),
            4 => Some(vec![FREE, 3]),
            _ => None,
        }
    };
    let mut owned = vec![false; 8];
    assert!(damaged(fat_sector_ids(&header, 3, &cycle, 8, &mut owned)).contains("loops"));

    // A DIFAT chain that starts past the end of the file.
    let none = |_: u32| -> Option<Vec<u32>> { None };
    let mut owned = vec![false; 8];
    assert!(damaged(fat_sector_ids(&header, 20, &none, 8, &mut owned)).contains("past its end"));

    // A DIFAT sector inside the file whose contents can't be read.
    let mut owned = vec![false; 8];
    assert!(damaged(fat_sector_ids(&header, 5, &none, 8, &mut owned)).contains("past its end"));

    // The same FAT sector listed twice.
    let twice: Vec<u32> = [1, 1]
        .into_iter()
        .chain(std::iter::repeat_n(FREE, 107))
        .collect();
    let mut owned = vec![false; 8];
    assert!(damaged(fat_sector_ids(&twice, END, &none, 8, &mut owned)).contains("twice"));

    // A FAT sector past the end of the file.
    let far: Vec<u32> = [9]
        .into_iter()
        .chain(std::iter::repeat_n(FREE, 108))
        .collect();
    let mut owned = vec![false; 8];
    assert!(damaged(fat_sector_ids(&far, END, &none, 8, &mut owned)).contains("past its end"));
}

/// No sibling or child.
const NONE: u32 = 0xFFFF_FFFF;

#[test]
fn a_valid_directory_tree_is_walked() {
    // Root (0) → child 2; 2 has left sibling 1 and right sibling 3; 3 is a storage
    // holding 4.
    let links = [
        (NONE, NONE, 2),
        (NONE, NONE, NONE),
        (1, 3, NONE),
        (NONE, NONE, 4),
        (NONE, NONE, NONE),
    ];
    let mut reached = walk_directory_tree(&links).unwrap();
    reached.sort_unstable();
    assert_eq!(reached, vec![1, 2, 3, 4]);
    // A root with no children reaches nothing.
    assert!(walk_directory_tree(&[(NONE, NONE, NONE)])
        .unwrap()
        .is_empty());
}

#[test]
fn a_directory_tree_that_loops_or_names_no_entry_is_refused() {
    // An entry reached twice: 1's left sibling is 1.
    assert!(damaged(walk_directory_tree(&[(NONE, NONE, 1), (1, NONE, NONE)])).contains("loops"));
    // Back to the root.
    assert!(damaged(walk_directory_tree(&[(NONE, NONE, 1), (NONE, 0, NONE)])).contains("loops"));
    // A link naming no entry.
    assert!(damaged(walk_directory_tree(&[(NONE, NONE, 7)])).contains("not there"));
    // No directory at all.
    assert!(damaged(walk_directory_tree(&[])).contains("no directory"));
}

#[test]
fn a_directory_tree_deeper_than_office_oxide_can_follow_is_too_deep() {
    // A chain of right siblings 0 → 1 → … → n, one level deeper each.
    let chain = |n: usize| -> Vec<(u32, u32, u32)> {
        (0..=n)
            .map(|i| {
                let next = if i < n { (i + 1) as u32 } else { NONE };
                if i == 0 {
                    (NONE, NONE, next)
                } else {
                    (NONE, next, NONE)
                }
            })
            .collect()
    };
    assert_eq!(
        walk_directory_tree(&chain(MAX_CFB_TREE_DEPTH))
            .unwrap()
            .len(),
        MAX_CFB_TREE_DEPTH
    );
    assert_eq!(
        walk_directory_tree(&chain(MAX_CFB_TREE_DEPTH + 1)),
        Err(CfbIssue::TooDeep)
    );
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

/// The app ships office_oxide's licence: its NOTICE entry says which of its two
/// licences Shodh takes and carries that licence's text.
#[test]
fn office_oxide_ships_with_the_licence_it_is_used_under() {
    // As checked out: line ends may be CRLF.
    let notice = include_str!("../../../../NOTICE").replace("\r\n", "\n");
    let entry = notice
        .split("\nOfficeOxide\n")
        .nth(1)
        .expect("NOTICE has an OfficeOxide entry");
    let entry = entry.split("\n\n").next().unwrap_or(entry);
    assert!(entry.contains("version 0.1.10"), "{entry}");
    assert!(entry.contains("used under MIT"), "{entry}");
    assert!(
        entry.contains("Permission is hereby granted, free of charge"),
        "{entry}"
    );
    assert!(
        entry.contains("THE SOFTWARE IS PROVIDED \"AS IS\""),
        "{entry}"
    );
}
