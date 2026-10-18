//! The text of Word (`word/document.xml`) and PowerPoint
//! (`ppt/slides/slideN.xml`) parts, read with an XML parser: only the text
//! runs (`w:t`, `a:t`) are kept, never tags, properties or tab-stop lists;
//! entities are decoded; a run's `w:tab` is a tab and `w:br`/`w:cr` (`a:br`)
//! a line break; each paragraph is a line; of the branches of an
//! `mc:AlternateContent` only one is read (the first `mc:Choice`, else the
//! `mc:Fallback`), so a text box Word stores twice is read once. Word tables
//! are kept as rows of cells.
//!
//! Elements are matched by namespace and local name, so any prefix the file
//! binds them to reads, in the transitional and in the strict schema.

use roxmltree::{Document, Node, ParsingOptions};

use super::limits::{self, TooLarge};

/// WordprocessingML: transitional and strict.
const W_NS: &[&str] = &[
    "http://schemas.openxmlformats.org/wordprocessingml/2006/main",
    "http://purl.oclc.org/ooxml/wordprocessingml/main",
];

/// DrawingML: transitional and strict.
const A_NS: &[&str] = &[
    "http://schemas.openxmlformats.org/drawingml/2006/main",
    "http://purl.oclc.org/ooxml/drawingml/main",
];

/// Markup compatibility.
const MC_NS: &[&str] = &["http://schemas.openxmlformats.org/markup-compatibility/2006"];

/// Word elements whose content is never text of the document: properties,
/// deleted or moved-away text, field instructions.
const W_SKIPPED: &[&str] = &[
    "pPr",
    "rPr",
    "sectPr",
    "sdtPr",
    "sdtEndPr",
    "tblPr",
    "tblGrid",
    "trPr",
    "tcPr",
    "del",
    "moveFrom",
    "delText",
    "instrText",
    "fldData",
];

/// DrawingML elements whose content is never text of the slide.
const A_SKIPPED: &[&str] = &["pPr", "rPr", "endParaRPr", "lstStyle", "bodyPr"];

/// What a Word document's body holds, in reading order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocxBlock {
    /// One line of text: a paragraph, or a paragraph of a text box.
    Paragraph(String),
    /// A table: its rows, each the text of its cells.
    Table(Vec<Vec<String>>),
}

/// Whether `node` is the element `name` of one of `namespaces`.
fn is(node: Node<'_, '_>, namespaces: &[&str], name: &str) -> bool {
    node.is_element()
        && node.tag_name().name() == name
        && node
            .tag_name()
            .namespace()
            .is_some_and(|ns| namespaces.contains(&ns))
}

/// Whether `node` is an element of one of `namespaces` named in `names`.
fn is_one_of(node: Node<'_, '_>, namespaces: &[&str], names: &[&str]) -> bool {
    names.iter().any(|name| is(node, namespaces, name))
}

/// The branch of an `mc:AlternateContent` that is read: its first
/// `mc:Choice`, else its `mc:Fallback`.
fn alternate_branch<'a, 'input>(node: Node<'a, 'input>) -> Option<Node<'a, 'input>> {
    node.children()
        .find(|c| is(*c, MC_NS, "Choice"))
        .or_else(|| node.children().find(|c| is(*c, MC_NS, "Fallback")))
}

/// The text of a text-run element (`w:t`, `a:t`), entities decoded.
fn run_text(node: Node<'_, '_>) -> String {
    node.children()
        .filter(|c| c.is_text())
        .filter_map(|c| c.text())
        .collect()
}

/// Lines being collected: those finished, and the one being written.
#[derive(Default)]
struct Lines {
    done: Vec<String>,
    current: String,
}

impl Lines {
    /// Ends the line being written; a line of whitespace only is dropped.
    fn flush(&mut self) {
        let line = std::mem::take(&mut self.current);
        if !line.trim().is_empty() {
            self.done.push(line);
        }
    }

    fn finish(mut self) -> Vec<String> {
        self.flush();
        self.done
    }
}

/// Deepest nesting of elements read. Office files nest a few dozen levels;
/// the walk recurses, so a deeper file (crafted: nesting costs a few bytes a
/// level and compresses to almost nothing) is refused rather than walked
/// until the thread's stack runs out.
pub const MAX_DEPTH: usize = 256;

/// Why a part could not be read.
#[derive(Debug, thiserror::Error)]
pub enum OfficeXmlError {
    #[error("not valid XML: {0}")]
    Invalid(roxmltree::Error),
    #[error(
        "its XML nests more than {} elements deep, which no Office file does",
        MAX_DEPTH
    )]
    TooDeep,
    #[error(transparent)]
    TooLarge(#[from] TooLarge),
}

type Walk = Result<(), OfficeXmlError>;

/// The part `xml` parsed, with at most [`limits::MAX_OFFICE_XML_NODES`]
/// nodes.
fn parse(xml: &str) -> Result<Document<'_>, OfficeXmlError> {
    let options = ParsingOptions {
        allow_dtd: false,
        nodes_limit: limits::MAX_OFFICE_XML_NODES,
    };
    Document::parse_with_options(xml, options).map_err(|e| match e {
        roxmltree::Error::NodesLimitReached => {
            OfficeXmlError::TooLarge(limits::office_xml_nodes_exceeded())
        }
        e => OfficeXmlError::Invalid(e),
    })
}

/// The depth below `depth`, refused past [`MAX_DEPTH`].
fn descend(depth: usize) -> Result<usize, OfficeXmlError> {
    if depth >= MAX_DEPTH {
        return Err(OfficeXmlError::TooDeep);
    }
    Ok(depth + 1)
}

/// The body of `word/document.xml` in reading order: its paragraphs (with
/// those of text boxes and content controls) and its tables.
pub fn docx_blocks(xml: &str) -> Result<Vec<DocxBlock>, OfficeXmlError> {
    let doc = parse(xml)?;
    let mut out = Vec::new();
    if let Some(body) = doc.root_element().children().find(|c| is(*c, W_NS, "body")) {
        docx_block_level(body, &mut out, 1)?;
    }
    Ok(out)
}

/// Paragraphs and tables under `parent` (at `depth`), through content
/// controls, custom XML and alternate content.
fn docx_block_level(parent: Node<'_, '_>, out: &mut Vec<DocxBlock>, depth: usize) -> Walk {
    let depth = descend(depth)?;
    for child in parent.children().filter(Node::is_element) {
        if is(child, W_NS, "p") {
            let mut lines = Lines::default();
            docx_inline(child, &mut lines, depth)?;
            out.extend(lines.finish().into_iter().map(DocxBlock::Paragraph));
        } else if is(child, W_NS, "tbl") {
            let rows = docx_table(child, depth)?;
            if !rows.is_empty() {
                out.push(DocxBlock::Table(rows));
            }
        } else if is(child, MC_NS, "AlternateContent") {
            if let Some(branch) = alternate_branch(child) {
                docx_block_level(branch, out, depth)?;
            }
        } else if !is_one_of(child, W_NS, W_SKIPPED) {
            docx_block_level(child, out, depth)?;
        }
    }
    Ok(())
}

/// The text under `node` (at `depth`) within a paragraph, into `lines`. A
/// paragraph nested in it (a text box's) is a line of its own, and a table
/// nested in it (in a table cell) a line per row.
fn docx_inline(node: Node<'_, '_>, lines: &mut Lines, depth: usize) -> Walk {
    let depth = descend(depth)?;
    for child in node.children().filter(Node::is_element) {
        if is(child, MC_NS, "AlternateContent") {
            if let Some(branch) = alternate_branch(child) {
                docx_inline(branch, lines, depth)?;
            }
            continue;
        }
        if !child
            .tag_name()
            .namespace()
            .is_some_and(|ns| W_NS.contains(&ns))
        {
            // Drawings, shapes and VML: their text boxes hold Word paragraphs.
            docx_inline(child, lines, depth)?;
            continue;
        }
        match child.tag_name().name() {
            "t" => lines.current.push_str(&run_text(child)),
            "tab" if child.parent().is_some_and(|p| is(p, W_NS, "r")) => lines.current.push('\t'),
            "br" | "cr" => lines.current.push('\n'),
            "noBreakHyphen" => lines.current.push('-'),
            "p" => {
                lines.flush();
                docx_inline(child, lines, depth)?;
                lines.flush();
            }
            "tbl" => {
                lines.flush();
                for row in docx_table(child, depth)? {
                    lines.current = row.join("\t");
                    lines.flush();
                }
            }
            name if W_SKIPPED.contains(&name) => {}
            _ => docx_inline(child, lines, depth)?,
        }
    }
    Ok(())
}

/// The rows of a Word table (at `depth`), each the text of its cells (a
/// cell's lines joined by spaces). Rows without text are left out.
fn docx_table(table: Node<'_, '_>, depth: usize) -> Result<Vec<Vec<String>>, OfficeXmlError> {
    let mut rows = Vec::new();
    docx_rows(table, &mut rows, depth)?;
    rows.retain(|row| row.iter().any(|cell| !cell.trim().is_empty()));
    Ok(rows)
}

fn docx_rows(node: Node<'_, '_>, rows: &mut Vec<Vec<String>>, depth: usize) -> Walk {
    let depth = descend(depth)?;
    for child in node.children().filter(Node::is_element) {
        if is(child, W_NS, "tr") {
            let mut cells = Vec::new();
            docx_cells(child, &mut cells, depth)?;
            rows.push(cells);
        } else if is(child, MC_NS, "AlternateContent") {
            if let Some(branch) = alternate_branch(child) {
                docx_rows(branch, rows, depth)?;
            }
        } else if !is(child, W_NS, "tbl") && !is_one_of(child, W_NS, W_SKIPPED) {
            docx_rows(child, rows, depth)?;
        }
    }
    Ok(())
}

fn docx_cells(node: Node<'_, '_>, cells: &mut Vec<String>, depth: usize) -> Walk {
    let depth = descend(depth)?;
    for child in node.children().filter(Node::is_element) {
        if is(child, W_NS, "tc") {
            let mut lines = Lines::default();
            docx_inline(child, &mut lines, depth)?;
            let text = lines
                .finish()
                .iter()
                .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
                .collect::<Vec<_>>()
                .join(" ");
            cells.push(text);
        } else if is(child, MC_NS, "AlternateContent") {
            if let Some(branch) = alternate_branch(child) {
                docx_cells(branch, cells, depth)?;
            }
        } else if !is_one_of(child, W_NS, W_SKIPPED) {
            docx_cells(child, cells, depth)?;
        }
    }
    Ok(())
}

/// The text of `blocks`: a line per paragraph, a line per table row (its
/// cells separated by tabs).
pub fn docx_text(blocks: &[DocxBlock]) -> String {
    let mut lines: Vec<String> = Vec::new();
    for block in blocks {
        match block {
            DocxBlock::Paragraph(text) => lines.push(text.clone()),
            DocxBlock::Table(rows) => lines.extend(rows.iter().map(|row| row.join("\t"))),
        }
    }
    lines.join("\n")
}

/// The text of a slide: a line per paragraph, in the order of the slide's
/// shapes.
pub fn pptx_slide_text(xml: &str) -> Result<String, OfficeXmlError> {
    let doc = parse(xml)?;
    let mut lines = Lines::default();
    pptx_inline(doc.root_element(), &mut lines, 0)?;
    Ok(lines.finish().join("\n"))
}

fn pptx_inline(node: Node<'_, '_>, lines: &mut Lines, depth: usize) -> Walk {
    let depth = descend(depth)?;
    for child in node.children().filter(Node::is_element) {
        if is(child, MC_NS, "AlternateContent") {
            if let Some(branch) = alternate_branch(child) {
                pptx_inline(branch, lines, depth)?;
            }
        } else if is(child, A_NS, "t") {
            lines.current.push_str(&run_text(child));
        } else if is(child, A_NS, "br") {
            lines.current.push('\n');
        } else if is(child, A_NS, "p") {
            lines.flush();
            pptx_inline(child, lines, depth)?;
            lines.flush();
        } else if !is_one_of(child, A_NS, A_SKIPPED) {
            pptx_inline(child, lines, depth)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: &str = "xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" \
                     xmlns:mc=\"http://schemas.openxmlformats.org/markup-compatibility/2006\" \
                     xmlns:wps=\"http://schemas.microsoft.com/office/word/2010/wordprocessingShape\" \
                     xmlns:v=\"urn:schemas-microsoft-com:vml\"";

    fn document(body: &str) -> String {
        format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:document {W}><w:body>{body}<w:sectPr><w:pgSz w:w=\"12240\"/></w:sectPr></w:body></w:document>")
    }

    fn paragraphs(blocks: &[DocxBlock]) -> Vec<&str> {
        blocks
            .iter()
            .filter_map(|b| match b {
                DocxBlock::Paragraph(text) => Some(text.as_str()),
                DocxBlock::Table(_) => None,
            })
            .collect()
    }

    #[test]
    fn tab_stops_in_paragraph_properties_are_not_text() {
        let xml = document(
            "<w:p><w:pPr><w:tabs><w:tab w:val=\"left\" w:pos=\"720\"/></w:tabs><w:rPr><w:b/></w:rPr></w:pPr>\
             <w:r><w:rPr><w:b/></w:rPr><w:t>Name</w:t></w:r><w:r><w:tab/><w:t xml:space=\"preserve\">Asha Rao </w:t></w:r>\
             <w:r><w:br/><w:t>Second line</w:t></w:r></w:p>",
        );
        let blocks = docx_blocks(&xml).unwrap();
        assert_eq!(
            paragraphs(&blocks),
            ["Name\tAsha Rao \nSecond line"],
            "{blocks:?}"
        );
        let text = docx_text(&blocks);
        assert!(!text.contains('<') && !text.contains("w:"), "{text}");
    }

    #[test]
    fn entities_are_decoded_and_deleted_text_and_fields_left_out() {
        let xml = document(
            "<w:p><w:r><w:t>R&amp;D costs &lt; 5% &#8212; &quot;fine&quot;</w:t></w:r>\
             <w:del><w:r><w:delText>old words</w:delText></w:r></w:del>\
             <w:r><w:instrText> PAGE </w:instrText></w:r><w:ins><w:r><w:t> added</w:t></w:r></w:ins></w:p>",
        );
        let blocks = docx_blocks(&xml).unwrap();
        assert_eq!(
            paragraphs(&blocks),
            ["R&D costs < 5% \u{2014} \"fine\" added"]
        );
    }

    #[test]
    fn a_text_box_is_read_once_as_its_own_line() {
        let text_box = "<w:txbxContent><w:p><w:r><w:t>Boxed note</w:t></w:r></w:p></w:txbxContent>";
        let xml = document(&format!(
            "<w:p><w:r><w:t>Before the box.</w:t></w:r><w:r><mc:AlternateContent>\
             <mc:Choice Requires=\"wps\"><w:drawing><wps:wsp><wps:txbx>{text_box}</wps:txbx></wps:wsp></w:drawing></mc:Choice>\
             <mc:Fallback><w:pict><v:shape><v:textbox>{text_box}</v:textbox></v:shape></w:pict></mc:Fallback>\
             </mc:AlternateContent></w:r><w:r><w:t>After the box.</w:t></w:r></w:p>"
        ));
        let blocks = docx_blocks(&xml).unwrap();
        assert_eq!(
            paragraphs(&blocks),
            ["Before the box.", "Boxed note", "After the box."]
        );
    }

    #[test]
    fn a_fallback_is_read_when_there_is_no_choice() {
        let xml = document(
            "<w:p><w:r><mc:AlternateContent><mc:Fallback><w:t>only fallback</w:t></mc:Fallback>\
             </mc:AlternateContent></w:r></w:p>",
        );
        assert_eq!(paragraphs(&docx_blocks(&xml).unwrap()), ["only fallback"]);
    }

    #[test]
    fn tables_are_rows_of_cells_in_reading_order() {
        let cell = |text: &str| {
            format!("<w:tc><w:tcPr><w:tcW w:w=\"2000\"/></w:tcPr><w:p><w:r><w:t>{text}</w:t></w:r></w:p></w:tc>")
        };
        let xml = document(&format!(
            "<w:p><w:r><w:t>Quarterly sales</w:t></w:r></w:p>\
             <w:tbl><w:tblPr><w:tblW w:w=\"0\"/></w:tblPr><w:tblGrid><w:gridCol w:w=\"2000\"/></w:tblGrid>\
             <w:tr>{}{}</w:tr><w:tr>{}{}</w:tr><w:tr><w:tc><w:p/></w:tc></w:tr></w:tbl>\
             <w:sdt><w:sdtPr><w:alias w:val=\"x\"/></w:sdtPr><w:sdtContent><w:p><w:r><w:t>After</w:t></w:r></w:p></w:sdtContent></w:sdt>",
            cell("Region"),
            cell("Sales"),
            cell("North"),
            cell("1,200 &amp; rising"),
        ));
        let blocks = docx_blocks(&xml).unwrap();
        assert_eq!(
            blocks,
            [
                DocxBlock::Paragraph("Quarterly sales".to_string()),
                DocxBlock::Table(vec![
                    vec!["Region".to_string(), "Sales".to_string()],
                    vec!["North".to_string(), "1,200 & rising".to_string()],
                ]),
                DocxBlock::Paragraph("After".to_string()),
            ]
        );
        assert_eq!(
            docx_text(&blocks),
            "Quarterly sales\nRegion\tSales\nNorth\t1,200 & rising\nAfter"
        );
    }

    #[test]
    fn malformed_xml_is_an_error() {
        assert!(docx_blocks("<w:document><w:body><w:p>").is_err());
        assert!(pptx_slide_text("<p:sld><a:p>").is_err());
    }

    /// XML nested far deeper than any Office file (a crafted part: a few
    /// bytes a level, compressing to almost nothing) is refused with a
    /// reason, not walked until the stack overflows.
    #[test]
    fn deeply_nested_xml_is_refused_not_walked() {
        let levels = 100_000;
        let deep = format!(
            "{}<w:p><w:r><w:t>buried</w:t></w:r></w:p>{}",
            "<w:sdtContent>".repeat(levels),
            "</w:sdtContent>".repeat(levels)
        );
        let err = docx_blocks(&document(&deep)).unwrap_err();
        assert!(matches!(err, OfficeXmlError::TooDeep), "{err}");
        assert!(err.to_string().contains("nests more than 256"), "{err}");

        let deep_cell = format!(
            "<w:tbl><w:tr><w:tc>{}<w:p><w:r><w:t>x</w:t></w:r></w:p>{}</w:tc></w:tr></w:tbl>",
            "<w:sdtContent>".repeat(levels),
            "</w:sdtContent>".repeat(levels)
        );
        assert!(matches!(
            docx_blocks(&document(&deep_cell)),
            Err(OfficeXmlError::TooDeep)
        ));

        let slide = format!(
            "<p:sld xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\" \
             xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\">{}<a:p><a:r><a:t>x</a:t></a:r></a:p>{}</p:sld>",
            "<p:grpSp>".repeat(levels),
            "</p:grpSp>".repeat(levels)
        );
        assert!(matches!(
            pptx_slide_text(&slide),
            Err(OfficeXmlError::TooDeep)
        ));

        // Nesting an Office file has is read.
        let nested = format!(
            "{}<w:p><w:r><w:t>found</w:t></w:r></w:p>{}",
            "<w:sdtContent>".repeat(40),
            "</w:sdtContent>".repeat(40)
        );
        assert_eq!(
            docx_blocks(&document(&nested)).unwrap(),
            [DocxBlock::Paragraph("found".to_string())]
        );
    }

    #[test]
    fn slide_text_keeps_runs_and_breaks_but_not_tab_lists() {
        let xml = "<p:sld xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\" \
                   xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\"><p:cSld><p:spTree>\
                   <p:sp><p:txBody><a:bodyPr/><a:lstStyle/>\
                   <a:p><a:pPr><a:tabLst><a:tab pos=\"914400\" algn=\"l\"/></a:tabLst></a:pPr>\
                   <a:r><a:rPr lang=\"en-US\"/><a:t>Revenue &amp; costs</a:t></a:r><a:br/><a:r><a:t>by region</a:t></a:r>\
                   <a:endParaRPr lang=\"en-US\"/></a:p>\
                   <a:p><a:fld type=\"slidenum\"><a:t>7</a:t></a:fld></a:p></p:txBody></p:sp>\
                   </p:spTree></p:cSld></p:sld>";
        assert_eq!(
            pptx_slide_text(xml).unwrap(),
            "Revenue & costs\nby region\n7"
        );
    }
}
