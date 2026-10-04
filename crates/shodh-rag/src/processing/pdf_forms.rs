//! Interactive form extraction: AcroForm fields and XFA data.
//!
//! The text layer of a filled PDF form usually does not contain the filled-in
//! values: they live in the form's field dictionaries (`/V`), in the widgets'
//! appearance streams (`/AP /N`), or, for XFA forms, in the `datasets` XML packet.
//! This module reads all three and returns one typed [`FormField`] per field, with
//! the page and widget rectangle it is drawn at, so the parser can place it in the
//! structured document as a citable block.
//!
//! - The field hierarchy is walked through `/Kids`; names are fully qualified through
//!   the `/T` chain, and `/FT`, `/Ff`, `/V` and `/Opt` are inherited from ancestors.
//! - The label is the field's tooltip (`/TU`), else its partial name made readable.
//! - Values are typed: text, choice (multi-select joined), checkbox (`/AS` against the
//!   widget's on state, `/Opt` export values), radio group (the selected widget's
//!   export value), signature (present or not); push buttons carry no value.
//! - A text field with no `/V` falls back to the text drawn by its appearance stream.
//! - XFA `datasets` values are read as leaf elements of `xfa:data`, labelled with the
//!   template's field captions where the names match; values an AcroForm field
//!   already carries are not repeated.
//!
//! Coordinates are PDF user space (points, bottom-left origin), as in
//! [`super::document_model::BBox`].

use std::collections::{HashMap, HashSet};

use lopdf::{Dictionary, Document, Object, ObjectId};

use super::document_model::BBox;
use super::lopdf_parser::decode_pdf_string;

/// Deepest field hierarchy walked (guards against reference cycles in corrupt files).
const MAX_DEPTH: usize = 32;
/// Most fields read from one document.
const MAX_FIELDS: usize = 20_000;
/// Longest value kept per field, in characters.
const MAX_VALUE_CHARS: usize = 4_000;

/// `/Ff` bit of a radio button group (bit position 16, 1-based).
const FF_RADIO: i64 = 1 << 15;
/// `/Ff` bit of a push button (bit position 17, 1-based).
const FF_PUSHBUTTON: i64 = 1 << 16;
/// `/Ff` bit of a multi-select choice field (bit position 22, 1-based).
const FF_MULTISELECT: i64 = 1 << 21;

/// Why a form could not be read.
#[derive(Debug, thiserror::Error)]
pub enum FormError {
    #[error("cannot open PDF: {0}")]
    Open(String),
    #[error("PDF is encrypted with a password")]
    Encrypted,
}

/// What kind of field a value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldKind {
    Text,
    Choice,
    Checkbox,
    Radio,
    Signature,
    /// A key-value pair read from the page layout of a flattened (printed) form.
    Layout,
    /// A comment or note annotation.
    Comment,
}

impl FieldKind {
    pub fn name(self) -> &'static str {
        match self {
            FieldKind::Text => "text",
            FieldKind::Choice => "choice",
            FieldKind::Checkbox => "checkbox",
            FieldKind::Radio => "radio",
            FieldKind::Signature => "signature",
            FieldKind::Layout => "layout",
            FieldKind::Comment => "comment",
        }
    }
}

/// Where a field's value was read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueSource {
    /// The field's `/V` (or a widget's `/AS` state).
    Value,
    /// Text drawn by the widget's appearance stream (`/V` was empty).
    Appearance,
    /// The XFA `datasets` packet.
    Xfa,
    /// A comment annotation's `/Contents`.
    Annotation,
}

/// One form field with its value.
#[derive(Debug, Clone, PartialEq)]
pub struct FormField {
    /// Fully qualified field name (`parent.child`), or the XFA data path.
    pub name: String,
    /// Human-readable label: the tooltip, the XFA caption, or the readable name.
    pub label: String,
    pub kind: FieldKind,
    /// Display value. Checkboxes read `Yes`/`No` (or their export value when it is
    /// meaningful); signatures `Signed`/`Not signed`.
    pub value: String,
    /// 1-based page of the widget, when known.
    pub page: Option<u32>,
    /// Widget rectangle, when known.
    pub bbox: Option<BBox>,
    pub source: ValueSource,
}

/// Counts describing a document's form, for diagnostics and logs.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FormStats {
    pub acroform: bool,
    pub xfa: bool,
    /// Field dictionaries in the `/Fields` tree, terminal and non-terminal.
    pub field_nodes: usize,
    /// Terminal fields (those that carry a value).
    pub terminal_fields: usize,
    /// Fields emitted with a non-empty value.
    pub with_value: usize,
    /// Text fields whose value came from the appearance stream.
    pub from_appearance: usize,
    pub checkboxes: usize,
    pub radios: usize,
    pub choices: usize,
    pub signatures: usize,
    pub xfa_values: usize,
    pub comments: usize,
}

/// The fields of one document.
#[derive(Debug, Clone, Default)]
pub struct FormExtraction {
    pub fields: Vec<FormField>,
    pub stats: FormStats,
}

/// Bytes of the PDF itself: a header found after leading junk (some portals wrap
/// the file, e.g. in a serialized Java byte array) is moved to the start. PDF
/// readers accept a header anywhere in the first 1024 bytes.
pub fn strip_leading_junk(bytes: &[u8]) -> &[u8] {
    if bytes.starts_with(b"%PDF-") {
        return bytes;
    }
    let window = &bytes[..bytes.len().min(1024)];
    match window.windows(5).position(|w| w == b"%PDF-") {
        Some(at) => &bytes[at..],
        None => bytes,
    }
}

/// Read the form fields and comment annotations of a PDF. A PDF without a form
/// yields an empty extraction, not an error.
pub fn extract_forms(bytes: &[u8]) -> Result<FormExtraction, FormError> {
    let bytes = strip_leading_junk(bytes);
    let mut doc = Document::load_mem(bytes).map_err(|e| FormError::Open(e.to_string()))?;
    if doc.is_encrypted() {
        // Most "encrypted" PDFs only restrict editing: the user password is empty.
        doc.decrypt("").map_err(|_| FormError::Encrypted)?;
    }
    Ok(Extractor::new(&doc).run())
}

struct Extractor<'a> {
    doc: &'a Document,
    /// Page object → 1-based page number.
    page_numbers: HashMap<ObjectId, u32>,
    /// Annotation object → 1-based page number (from the pages' `/Annots`).
    annot_pages: HashMap<ObjectId, u32>,
    visited: HashSet<ObjectId>,
    out: FormExtraction,
}

/// Field attributes inherited down the hierarchy.
#[derive(Clone, Default)]
struct Inherited {
    name: String,
    field_type: Option<Vec<u8>>,
    flags: i64,
    value: Option<Object>,
    options: Option<Object>,
}

impl<'a> Extractor<'a> {
    fn new(doc: &'a Document) -> Self {
        let pages = doc.get_pages();
        let page_numbers: HashMap<ObjectId, u32> = pages.iter().map(|(n, id)| (*id, *n)).collect();
        let mut annot_pages = HashMap::new();
        for (number, page_id) in &pages {
            let Ok(page) = doc.get_dictionary(*page_id) else {
                continue;
            };
            if let Some(annots) =
                resolve(doc, page.get(b"Annots").ok()).and_then(|o| o.as_array().ok())
            {
                for annot in annots {
                    if let Ok(id) = annot.as_reference() {
                        annot_pages.entry(id).or_insert(*number);
                    }
                }
            }
        }
        Self {
            doc,
            page_numbers,
            annot_pages,
            visited: HashSet::new(),
            out: FormExtraction::default(),
        }
    }

    fn run(mut self) -> FormExtraction {
        let acroform = self
            .doc
            .catalog()
            .ok()
            .and_then(|c| resolve(self.doc, c.get(b"AcroForm").ok()))
            .and_then(|o| o.as_dict().ok());
        if let Some(acroform) = acroform {
            self.out.stats.acroform = true;
            if let Some(fields) =
                resolve(self.doc, acroform.get(b"Fields").ok()).and_then(|o| o.as_array().ok())
            {
                for field in fields {
                    if let Ok(id) = field.as_reference() {
                        self.walk(id, &Inherited::default(), 0);
                    }
                }
            }
            if let Some(xfa) = acroform.get(b"XFA").ok() {
                self.out.stats.xfa = true;
                self.read_xfa(xfa);
            }
        }
        self.read_comments();
        self.out.stats.with_value = self
            .out
            .fields
            .iter()
            .filter(|f| !f.value.trim().is_empty())
            .count();
        self.out
    }

    fn walk(&mut self, id: ObjectId, parent: &Inherited, depth: usize) {
        if depth > MAX_DEPTH || self.out.fields.len() >= MAX_FIELDS || !self.visited.insert(id) {
            return;
        }
        let Ok(dict) = self.doc.get_dictionary(id) else {
            return;
        };
        self.out.stats.field_nodes += 1;
        let mut here = parent.clone();
        if let Some(partial) = dict_text(self.doc, dict, b"T") {
            here.name = if here.name.is_empty() {
                partial
            } else {
                format!("{}.{partial}", here.name)
            };
        }
        if let Ok(ft) = dict.get(b"FT").and_then(Object::as_name) {
            here.field_type = Some(ft.to_vec());
        }
        if let Ok(ff) = dict.get(b"Ff").and_then(Object::as_i64) {
            here.flags = ff;
        }
        if let Some(v) = dict.get(b"V").ok().and_then(|v| resolve(self.doc, Some(v))) {
            here.value = Some(v.clone());
        }
        if let Some(opt) = dict
            .get(b"Opt")
            .ok()
            .and_then(|v| resolve(self.doc, Some(v)))
        {
            here.options = Some(opt.clone());
        }
        let kids: Vec<ObjectId> = resolve(self.doc, dict.get(b"Kids").ok())
            .and_then(|o| o.as_array().ok())
            .map(|a| a.iter().filter_map(|k| k.as_reference().ok()).collect())
            .unwrap_or_default();
        // Kids with a partial name are fields; kids without one are this field's widgets.
        let child_fields: Vec<ObjectId> = kids
            .iter()
            .copied()
            .filter(|k| {
                self.doc
                    .get_dictionary(*k)
                    .is_ok_and(|d| d.has(b"T") || d.has(b"Kids"))
            })
            .collect();
        if !child_fields.is_empty() {
            for kid in child_fields {
                self.walk(kid, &here, depth + 1);
            }
            return;
        }
        let widgets: Vec<ObjectId> = if kids.is_empty() { vec![id] } else { kids };
        let tooltip = dict_text(self.doc, dict, b"TU");
        self.terminal(&here, tooltip, &widgets);
    }

    fn terminal(&mut self, field: &Inherited, tooltip: Option<String>, widgets: &[ObjectId]) {
        self.out.stats.terminal_fields += 1;
        let label = tooltip
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| readable_name(&field.name));
        let first = widgets.first().copied();
        let Some(field_type) = field.field_type.as_deref() else {
            return;
        };
        let (kind, value, source, widget) = match field_type {
            b"Tx" => {
                let from_v = field.value.as_ref().and_then(|v| object_text(self.doc, v));
                match from_v.filter(|v| !v.trim().is_empty()) {
                    Some(v) => (FieldKind::Text, v, ValueSource::Value, first),
                    None => {
                        let drawn = widgets
                            .iter()
                            .find_map(|w| self.appearance_text(*w).map(|t| (t, *w)));
                        match drawn {
                            Some((text, w)) => {
                                self.out.stats.from_appearance += 1;
                                (FieldKind::Text, text, ValueSource::Appearance, Some(w))
                            }
                            None => (FieldKind::Text, String::new(), ValueSource::Value, first),
                        }
                    }
                }
            }
            b"Ch" => {
                self.out.stats.choices += 1;
                let value = field
                    .value
                    .as_ref()
                    .map(|v| self.choice_value(v, field.options.as_ref(), field.flags))
                    .unwrap_or_default();
                (FieldKind::Choice, value, ValueSource::Value, first)
            }
            b"Btn" if field.flags & FF_PUSHBUTTON != 0 => return,
            b"Btn" if field.flags & FF_RADIO != 0 => {
                self.out.stats.radios += 1;
                let (value, widget) = self.radio_value(field, widgets);
                (
                    FieldKind::Radio,
                    value,
                    ValueSource::Value,
                    widget.or(first),
                )
            }
            b"Btn" => {
                self.out.stats.checkboxes += 1;
                let (value, widget) = self.checkbox_value(field, widgets);
                (
                    FieldKind::Checkbox,
                    value,
                    ValueSource::Value,
                    widget.or(first),
                )
            }
            b"Sig" => {
                self.out.stats.signatures += 1;
                (
                    FieldKind::Signature,
                    self.signature_value(field.value.as_ref()),
                    ValueSource::Value,
                    first,
                )
            }
            _ => return,
        };
        let (page, bbox) = widget.map(|w| self.widget_place(w)).unwrap_or((None, None));
        self.out.fields.push(FormField {
            name: field.name.clone(),
            label,
            kind,
            value: clip(&collapse_value(&value)),
            page,
            bbox,
            source,
        });
    }

    /// Page and rectangle of a widget annotation.
    fn widget_place(&self, widget: ObjectId) -> (Option<u32>, Option<BBox>) {
        let Ok(dict) = self.doc.get_dictionary(widget) else {
            return (None, None);
        };
        let page = dict
            .get(b"P")
            .ok()
            .and_then(|p| p.as_reference().ok())
            .and_then(|p| self.page_numbers.get(&p).copied())
            .or_else(|| self.annot_pages.get(&widget).copied());
        (page, rect_of(self.doc, dict))
    }

    /// Text drawn by a widget's normal appearance stream, decoded as far as the
    /// strings allow (simple encodings and UTF-16; not CID fonts without a Unicode map).
    fn appearance_text(&self, widget: ObjectId) -> Option<String> {
        let dict = self.doc.get_dictionary(widget).ok()?;
        let ap = resolve(self.doc, dict.get(b"AP").ok())?.as_dict().ok()?;
        let normal = resolve(self.doc, ap.get(b"N").ok())?;
        let stream = normal.as_stream().ok()?;
        let bytes = stream_bytes(stream)?;
        let text = content_text(&bytes);
        (!text.trim().is_empty()).then_some(text)
    }

    fn choice_value(&self, value: &Object, options: Option<&Object>, flags: i64) -> String {
        let display = |export: String| -> String {
            options
                .and_then(|o| o.as_array().ok())
                .and_then(|opts| {
                    opts.iter().find_map(|opt| {
                        let pair = resolve(self.doc, Some(opt))?.as_array().ok()?;
                        let exp = object_text(self.doc, pair.first()?)?;
                        let shown = object_text(self.doc, pair.get(1)?)?;
                        (exp == export).then_some(shown)
                    })
                })
                .unwrap_or(export)
        };
        match value {
            Object::Array(items) if flags & FF_MULTISELECT != 0 || items.len() > 1 => items
                .iter()
                .filter_map(|i| object_text(self.doc, i))
                .map(display)
                .collect::<Vec<_>>()
                .join(", "),
            Object::Array(items) => items
                .iter()
                .find_map(|i| object_text(self.doc, i))
                .map(display)
                .unwrap_or_default(),
            other => object_text(self.doc, other)
                .map(display)
                .unwrap_or_default(),
        }
    }

    /// The on-state names of a button widget (the keys of `/AP /N` other than `Off`).
    fn on_state(&self, widget: ObjectId) -> Option<Vec<u8>> {
        let dict = self.doc.get_dictionary(widget).ok()?;
        let ap = resolve(self.doc, dict.get(b"AP").ok())?.as_dict().ok()?;
        let normal = resolve(self.doc, ap.get(b"N").ok())?.as_dict().ok()?;
        normal
            .iter()
            .map(|(k, _)| k.clone())
            .find(|k| k.as_slice() != b"Off")
    }

    fn appearance_state(&self, widget: ObjectId) -> Option<Vec<u8>> {
        let dict = self.doc.get_dictionary(widget).ok()?;
        dict.get(b"AS").ok()?.as_name().ok().map(<[u8]>::to_vec)
    }

    /// The export value of the widget at `index`: `/Opt[index]` when the field has
    /// `/Opt` (needed when on-state names are generic, such as `0`, `1`), else the
    /// on-state name.
    fn export_value(&self, field: &Inherited, index: usize, state: &[u8]) -> String {
        field
            .options
            .as_ref()
            .and_then(|o| o.as_array().ok())
            .and_then(|opts| opts.get(index))
            .and_then(|o| object_text(self.doc, o))
            .unwrap_or_else(|| name_text(state))
    }

    fn checkbox_value(
        &self,
        field: &Inherited,
        widgets: &[ObjectId],
    ) -> (String, Option<ObjectId>) {
        let selected = field
            .value
            .as_ref()
            .and_then(|v| v.as_name().ok())
            .map(<[u8]>::to_vec);
        for (index, widget) in widgets.iter().enumerate() {
            let on = self.on_state(*widget);
            let state = self.appearance_state(*widget);
            let checked = match (&selected, &on, &state) {
                (Some(v), Some(on), _) => v.as_slice() != b"Off" && v == on,
                (Some(v), None, _) => v.as_slice() != b"Off",
                (None, _, Some(s)) => s.as_slice() != b"Off",
                (None, _, None) => false,
            };
            if checked {
                let state = on.or(selected.clone()).unwrap_or_else(|| b"Yes".to_vec());
                let export = self.export_value(field, index, &state);
                return (checkbox_text(&export), Some(*widget));
            }
        }
        ("No".to_string(), widgets.first().copied())
    }

    fn radio_value(&self, field: &Inherited, widgets: &[ObjectId]) -> (String, Option<ObjectId>) {
        let selected = field
            .value
            .as_ref()
            .and_then(|v| v.as_name().ok())
            .map(<[u8]>::to_vec)
            .filter(|v| v.as_slice() != b"Off");
        for (index, widget) in widgets.iter().enumerate() {
            let on = self.on_state(*widget);
            let state = self
                .appearance_state(*widget)
                .filter(|s| s.as_slice() != b"Off");
            let chosen = match (&selected, &on) {
                (Some(v), Some(on)) => v == on,
                (Some(_), None) => false,
                (None, _) => state.is_some(),
            };
            if chosen {
                let state = on.or(state).unwrap_or_default();
                return (self.export_value(field, index, &state), Some(*widget));
            }
        }
        (String::new(), None)
    }

    fn signature_value(&self, value: Option<&Object>) -> String {
        let Some(dict) = value.and_then(|v| v.as_dict().ok()) else {
            return "Not signed".to_string();
        };
        let mut out = "Signed".to_string();
        if let Some(name) = dict_text(self.doc, dict, b"Name").filter(|n| !n.trim().is_empty()) {
            out.push_str(" by ");
            out.push_str(name.trim());
        }
        if let Some(date) = dict_text(self.doc, dict, b"M").and_then(|m| pdf_date(&m)) {
            out.push_str(" on ");
            out.push_str(&date);
        }
        out
    }

    fn read_xfa(&mut self, xfa: &Object) {
        let Some(packets) = xfa_packets(self.doc, xfa) else {
            return;
        };
        let captions = packets
            .iter()
            .find(|(name, _)| name == "template")
            .map(|(_, xml)| xfa_captions(xml))
            .unwrap_or_default();
        let Some((_, datasets)) = packets.iter().find(|(name, _)| name == "datasets") else {
            return;
        };
        let known: HashSet<(String, String)> = self
            .out
            .fields
            .iter()
            .map(|f| {
                (
                    leaf_name(&f.name).to_lowercase(),
                    f.value.trim().to_string(),
                )
            })
            .collect();
        for (path, value) in xfa_values(datasets) {
            if self.out.fields.len() >= MAX_FIELDS {
                break;
            }
            let leaf = leaf_name(&path).to_string();
            if known.contains(&(leaf.to_lowercase(), value.trim().to_string())) {
                continue;
            }
            let label = captions
                .get(&leaf)
                .cloned()
                .unwrap_or_else(|| readable_name(&leaf));
            self.out.stats.xfa_values += 1;
            self.out.fields.push(FormField {
                name: path,
                label,
                kind: FieldKind::Text,
                value: clip(&collapse_value(&value)),
                page: None,
                bbox: None,
                source: ValueSource::Xfa,
            });
        }
    }

    /// Comment-like annotations (notes, free text, markup) with `/Contents`.
    fn read_comments(&mut self) {
        let mut annots: Vec<(ObjectId, u32)> =
            self.annot_pages.iter().map(|(a, p)| (*a, *p)).collect();
        annots.sort_by_key(|(id, page)| (*page, *id));
        for (id, page) in annots {
            let Ok(dict) = self.doc.get_dictionary(id) else {
                continue;
            };
            let subtype = dict
                .get(b"Subtype")
                .and_then(Object::as_name)
                .unwrap_or(b"");
            if matches!(subtype, b"Widget" | b"Link" | b"Popup") {
                continue;
            }
            let Some(contents) =
                dict_text(self.doc, dict, b"Contents").filter(|c| !c.trim().is_empty())
            else {
                continue;
            };
            self.out.stats.comments += 1;
            self.out.fields.push(FormField {
                name: format!("annotation-{}-{}", id.0, id.1),
                label: "Comment".to_string(),
                kind: FieldKind::Comment,
                value: clip(&collapse_value(&contents)),
                page: Some(page),
                bbox: rect_of(self.doc, dict),
                source: ValueSource::Annotation,
            });
        }
    }
}

fn resolve<'a>(doc: &'a Document, obj: Option<&'a Object>) -> Option<&'a Object> {
    let obj = obj?;
    match obj {
        Object::Reference(id) => doc.get_object(*id).ok(),
        other => Some(other),
    }
}

/// A string or name entry of a dictionary as text.
fn dict_text(doc: &Document, dict: &Dictionary, key: &[u8]) -> Option<String> {
    object_text(doc, resolve(doc, dict.get(key).ok())?)
}

fn object_text(doc: &Document, obj: &Object) -> Option<String> {
    match resolve(doc, Some(obj))? {
        Object::String(bytes, _) => Some(decode_pdf_string(bytes)),
        Object::Name(bytes) => Some(name_text(bytes)),
        Object::Integer(i) => Some(i.to_string()),
        Object::Real(r) => Some(r.to_string()),
        Object::Stream(stream) => stream_bytes(stream).map(|b| decode_pdf_string(&b)),
        _ => None,
    }
}

/// A PDF name as text, with `#xx` escapes decoded.
fn name_text(bytes: &[u8]) -> String {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'#' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&String::from_utf8_lossy(&bytes[i + 1..i + 3]), 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    decode_pdf_string(&out)
}

fn rect_of(doc: &Document, dict: &Dictionary) -> Option<BBox> {
    let rect = resolve(doc, dict.get(b"Rect").ok())?.as_array().ok()?;
    let n: Vec<f32> = rect
        .iter()
        .filter_map(|v| resolve(doc, Some(v)).and_then(|v| v.as_float().ok()))
        .collect();
    (n.len() == 4 && n.iter().all(|v| v.is_finite()))
        .then(|| BBox::new(n[0], n[1], n[2], n[3]).rounded())
}

/// Decoded bytes of a stream (unfiltered streams are returned as stored).
fn stream_bytes(stream: &lopdf::Stream) -> Option<Vec<u8>> {
    if stream.dict.has(b"Filter") {
        stream.decompressed_content().ok()
    } else {
        Some(stream.content.clone())
    }
}

/// Text shown by a content stream: the string operands of `Tj`, `TJ`, `'` and `"`,
/// one line per text object or explicit line move.
pub(crate) fn content_text(bytes: &[u8]) -> String {
    let Ok(content) = lopdf::content::Content::decode(bytes) else {
        return String::new();
    };
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    let flush = |current: &mut String, lines: &mut Vec<String>| {
        let line = current.trim().to_string();
        if !line.is_empty() {
            lines.push(line);
        }
        current.clear();
    };
    for op in &content.operations {
        match op.operator.as_str() {
            "Tj" | "'" | "\"" => {
                if matches!(op.operator.as_str(), "'" | "\"") {
                    flush(&mut current, &mut lines);
                }
                if let Some(Object::String(bytes, _)) = op.operands.last() {
                    current.push_str(&decode_pdf_string(bytes));
                }
            }
            "TJ" => {
                if let Some(Object::Array(items)) = op.operands.first() {
                    for item in items {
                        match item {
                            Object::String(bytes, _) => current.push_str(&decode_pdf_string(bytes)),
                            // A large negative kern is a word space.
                            Object::Integer(k) if *k < -200 => current.push(' '),
                            Object::Real(k) if *k < -200.0 => current.push(' '),
                            _ => {}
                        }
                    }
                }
            }
            "Td" | "TD" | "T*" | "ET" => flush(&mut current, &mut lines),
            _ => {}
        }
    }
    flush(&mut current, &mut lines);
    lines.retain(|l| l.chars().any(|c| !c.is_control()));
    lines.join(" ")
}

/// The XFA packets of an AcroForm `/XFA` entry: a single stream (the whole XDP) or
/// an array of `(name) stream` pairs. Returns `(packet name, xml)`.
fn xfa_packets(doc: &Document, xfa: &Object) -> Option<Vec<(String, String)>> {
    match resolve(doc, Some(xfa))? {
        Object::Array(items) => {
            let mut out = Vec::new();
            for pair in items.chunks(2) {
                let [name, stream] = pair else { continue };
                let Some(name) = object_text(doc, name) else {
                    continue;
                };
                let Some(stream) = resolve(doc, Some(stream)).and_then(|s| s.as_stream().ok())
                else {
                    continue;
                };
                if let Some(bytes) = stream_bytes(stream) {
                    out.push((name, String::from_utf8_lossy(&bytes).into_owned()));
                }
            }
            Some(out)
        }
        Object::Stream(stream) => {
            let xml = String::from_utf8_lossy(&stream_bytes(stream)?).into_owned();
            Some(split_xdp(&xml))
        }
        _ => None,
    }
}

/// Split a whole XDP document into its top-level packets (`template`, `datasets`, ...).
fn split_xdp(xml: &str) -> Vec<(String, String)> {
    let Ok(doc) = roxmltree::Document::parse(xml) else {
        return Vec::new();
    };
    doc.root_element()
        .children()
        .filter(|n| n.is_element())
        .map(|n| (n.tag_name().name().to_string(), xml[n.range()].to_string()))
        .collect()
}

/// Leaf values of the `data` element of an XFA `datasets` packet, as
/// `(dotted path below data, value)` in document order.
fn xfa_values(datasets: &str) -> Vec<(String, String)> {
    let Ok(doc) = roxmltree::Document::parse(datasets) else {
        return Vec::new();
    };
    let Some(data) = doc
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "data")
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for node in data.descendants().filter(|n| n.is_element() && *n != data) {
        if node.children().any(|c| c.is_element()) {
            continue;
        }
        let value: String = node.text().unwrap_or("").trim().to_string();
        if value.is_empty() {
            continue;
        }
        let mut parts: Vec<&str> = node
            .ancestors()
            .take_while(|a| *a != data)
            .filter(|a| a.is_element())
            .map(|a| a.tag_name().name())
            .collect();
        parts.reverse();
        out.push((parts.join("."), value));
    }
    out
}

/// Field name → caption text from an XFA `template` packet.
fn xfa_captions(template: &str) -> HashMap<String, String> {
    let Ok(doc) = roxmltree::Document::parse(template) else {
        return HashMap::new();
    };
    let mut out = HashMap::new();
    for field in doc
        .descendants()
        .filter(|n| n.is_element() && matches!(n.tag_name().name(), "field" | "exclGroup"))
    {
        let Some(name) = field.attribute("name") else {
            continue;
        };
        let caption = field
            .children()
            .find(|c| c.is_element() && c.tag_name().name() == "caption")
            .map(|c| {
                c.descendants()
                    .filter(|t| t.is_text())
                    .filter_map(|t| t.text())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .map(|t| collapse_value(&t))
            .filter(|t| !t.is_empty());
        if let Some(caption) = caption {
            out.entry(name.to_string()).or_insert(caption);
        }
    }
    out
}

/// The last segment of a dotted field name, without array indices (`a[0].b[2]` → `b`).
fn leaf_name(name: &str) -> &str {
    let last = name.rsplit('.').next().unwrap_or(name);
    last.split('[').next().unwrap_or(last)
}

/// A field name made readable: its last segment, array indices dropped, separators and
/// camel case turned into spaces (`applicant.grossSalary[0]` → `gross Salary`).
pub fn readable_name(name: &str) -> String {
    let leaf = leaf_name(name);
    let mut out = String::with_capacity(leaf.len() + 4);
    let mut prev: Option<char> = None;
    for c in leaf.chars() {
        let c = if c == '_' || c == '-' { ' ' } else { c };
        if c.is_uppercase() && prev.is_some_and(|p| p.is_lowercase()) {
            out.push(' ');
        }
        out.push(c);
        prev = Some(c);
    }
    let out = collapse_value(&out);
    if out.is_empty() {
        name.to_string()
    } else {
        out
    }
}

fn checkbox_text(export: &str) -> String {
    let generic = ["yes", "on", "1", "true", "x", "checked", "off"];
    if export.trim().is_empty() || generic.contains(&export.trim().to_lowercase().as_str()) {
        "Yes".to_string()
    } else {
        format!("Yes ({})", export.trim())
    }
}

/// `D:20240131120000+05'30'` → `2024-01-31`.
fn pdf_date(raw: &str) -> Option<String> {
    let digits: String = raw
        .trim_start_matches("D:")
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    (digits.len() >= 8).then(|| format!("{}-{}-{}", &digits[0..4], &digits[4..6], &digits[6..8]))
}

fn collapse_value(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn clip(text: &str) -> String {
    if text.chars().count() <= MAX_VALUE_CHARS {
        text.to_string()
    } else {
        let mut out: String = text.chars().take(MAX_VALUE_CHARS - 1).collect();
        out.push('…');
        out
    }
}
