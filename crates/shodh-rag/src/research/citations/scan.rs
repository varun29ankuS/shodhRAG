//! What one library PDF contributes to the graph, read from its parsed layout: its
//! identity, its references, and the headings and captions the deterministic method and
//! dataset rules read. A scan is stored in `shodh.db` per file fingerprint (size and
//! modification time), so a rebuild parses only files that changed.

use serde::{Deserialize, Serialize};

use super::identity::{identify, LocalIdentity, PdfInfo};
use super::reference::{parse_reference, ParsedReference};
use super::segment::{segment, BibBlock, EntryRegion, RejectReason};
use super::text::clean_block_text;
use crate::processing::document_model::{BlockKind, StructuredDocument};

/// Version of the scan rules; a stored scan of another version is redone.
pub const SCAN_VERSION: &str = "citation-scan/1";
/// Most references kept per paper (a survey's bibliography beyond this is cut).
pub const MAX_REFERENCES: usize = 400;
/// Most headings and captions kept per paper.
const MAX_LABELS: usize = 200;

/// One reference of a library paper.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScannedReference {
    /// Position in the bibliography (0-based).
    pub index: usize,
    pub page: Option<u32>,
    #[serde(default)]
    pub regions: Vec<EntryRegion>,
    pub parsed: ParsedReference,
}

/// The scan of one PDF.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaperScan {
    pub version: String,
    /// The file as spelled on disk.
    pub file_path: String,
    pub identity: LocalIdentity,
    pub references: Vec<ScannedReference>,
    /// Bibliography blocks or texts not used, per reason.
    pub rejected: Vec<(RejectReason, usize)>,
    /// References dropped beyond [`MAX_REFERENCES`].
    #[serde(default)]
    pub truncated: usize,
    /// Section headings, in order.
    pub headings: Vec<String>,
    /// Figure and table captions, in order.
    pub captions: Vec<String>,
    /// Pages in the document.
    pub pages: usize,
}

impl PaperScan {
    /// References that identify a work (see [`ParsedReference::is_identifiable`]).
    pub fn identifiable(&self) -> impl Iterator<Item = &ScannedReference> {
        self.references
            .iter()
            .filter(|r| r.parsed.is_identifiable())
    }
}

/// Scans a parsed PDF. `file_path` is the file as spelled on disk; `info` its Info
/// dictionary.
pub fn scan_document(doc: &StructuredDocument, info: &PdfInfo, file_path: &str) -> PaperScan {
    let file_name = crate::research::file_name(file_path);
    let identity = identify(doc, info, &file_name);
    let blocks: Vec<BibBlock> = doc
        .blocks
        .iter()
        .filter(|b| matches!(b.kind, BlockKind::ReferenceEntry))
        .map(|b| BibBlock {
            text: b.text.clone(),
            page: b.page,
            bbox: b.bbox,
            section_path: b.section_path.clone(),
        })
        .collect();
    let segmentation = segment(&blocks);
    let mut rejected: std::collections::BTreeMap<RejectReason, usize> = Default::default();
    for (reason, _) in &segmentation.rejected {
        *rejected.entry(*reason).or_insert(0) += 1;
    }
    let total = segmentation.entries.len();
    let references: Vec<ScannedReference> = segmentation
        .entries
        .into_iter()
        .take(MAX_REFERENCES)
        .enumerate()
        .map(|(index, entry)| ScannedReference {
            index,
            page: entry.page,
            regions: entry.regions,
            parsed: parse_reference(&entry.text),
        })
        .collect();
    let mut headings = Vec::new();
    let mut captions = Vec::new();
    for block in &doc.blocks {
        match &block.kind {
            BlockKind::Heading { .. } if headings.len() < MAX_LABELS => {
                headings.push(clean_block_text(&block.text));
            }
            BlockKind::Figure { caption } if captions.len() < MAX_LABELS => {
                captions.push(clean_block_text(caption));
            }
            BlockKind::Table {
                caption: Some(caption),
                ..
            } if captions.len() < MAX_LABELS => {
                captions.push(clean_block_text(caption));
            }
            _ => {}
        }
    }
    PaperScan {
        version: SCAN_VERSION.to_string(),
        file_path: file_path.to_string(),
        identity,
        truncated: total.saturating_sub(references.len()),
        references,
        rejected: rejected.into_iter().collect(),
        headings,
        captions,
        pages: doc.pages.len(),
    }
}
