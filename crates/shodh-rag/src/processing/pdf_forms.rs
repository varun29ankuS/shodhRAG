//! Interactive form extraction: AcroForm fields and comment annotations.
//!
//! The text layer of a filled PDF form usually does not contain the filled-in
//! values: they live in the form's field dictionaries (`/V`, `/AS`). This module
//! reads them and returns one typed [`FormField`] per field, with the page and
//! widget rectangle it is drawn at, so the parser can place it in the structured
//! document as a citable block.
//!
//! - The field hierarchy is walked through `/Kids`; names are fully qualified through
//!   the `/T` chain, and `/FT`, `/Ff`, `/V` and `/Opt` are inherited from ancestors.
//! - The label is the field's tooltip (`/TU`), else its partial name made readable.
//! - Values are typed: text, choice (multi-select joined), checkbox (`/AS` against the
//!   widget's on state, `/Opt` export values), radio group (the selected widget's
//!   export value), signature (present or not); push buttons carry no value.
//! - Comment annotations (notes, free text, markup) with `/Contents` are read too.
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
    /// A comment annotation's `/Contents`.
    Annotation,
}

/// One form field with its value.
#[derive(Debug, Clone, PartialEq)]
pub struct FormField {
    /// Fully qualified field name (`parent.child`).
    pub name: String,
    /// Human-readable label: the tooltip, else the readable field name.
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
    /// Field dictionaries in the `/Fields` tree, terminal and non-terminal.
    pub field_nodes: usize,
    /// Terminal fields (those that carry a value).
    pub terminal_fields: usize,
    /// Fields emitted with a non-empty value.
    pub with_value: usize,
    pub checkboxes: usize,
    pub radios: usize,
    pub choices: usize,
    pub signatures: usize,
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
                let value = field
                    .value
                    .as_ref()
                    .and_then(|v| object_text(self.doc, v))
                    .unwrap_or_default();
                (FieldKind::Text, value, ValueSource::Value, first)
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

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::processing::pdf_fixtures::{build_pdf, text};
    use lopdf::{dictionary, Stream};

    fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Object {
        Object::Array(vec![x0.into(), y0.into(), x1.into(), y1.into()])
    }

    fn string(s: &str) -> Object {
        Object::string_literal(s.as_bytes().to_vec())
    }

    /// Appearance stream dictionary with the given on state (and `Off`).
    fn states(doc: &mut Document, on: &str) -> Object {
        let on_id = doc.add_object(Stream::new(dictionary! {}, b"0 g".to_vec()));
        let off_id = doc.add_object(Stream::new(dictionary! {}, Vec::new()));
        let mut normal = Dictionary::new();
        normal.set(on.as_bytes().to_vec(), on_id);
        normal.set("Off", off_id);
        Object::Dictionary(dictionary! { "N" => normal })
    }

    /// A two-page tax-return-like PDF with an AcroForm: a field hierarchy with
    /// tooltips, a checkbox with an `/Opt` export value, an unchecked checkbox, a
    /// radio group, a multi-select list, a push button, a field on page 2 known only
    /// from the page's `/Annots`, a text widget with an appearance stream drawing its
    /// value, an empty field and a comment.
    pub(crate) fn form_pdf() -> Vec<u8> {
        let base = build_pdf(
            &[
                vec![
                    text(72.0, 740.0, 14.0, "Applicant details"),
                    text(72.0, 700.0, 10.0, "Full name"),
                    text(72.0, 670.0, 10.0, "Permanent account number"),
                    text(72.0, 600.0, 10.0, "Choices made by the applicant follow."),
                ],
                vec![text(72.0, 740.0, 10.0, "Employment")],
            ],
            None,
        );
        let mut doc = Document::load_mem(&base).expect("base pdf");
        let pages: Vec<ObjectId> = doc.get_pages().values().copied().collect();
        let (p1, p2) = (pages[0], pages[1]);
        let mut annots1: Vec<Object> = Vec::new();
        let mut annots2: Vec<Object> = Vec::new();

        let applicant = doc.new_object_id();
        let name_ap = doc.add_object(Stream::new(
            dictionary! {},
            b"BT /F1 10 Tf 2 4 Td (Asha Verma) Tj ET".to_vec(),
        ));
        let name = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Widget", "Parent" => applicant,
            "T" => string("name[0]"), "TU" => string("Full name of the applicant"),
            "FT" => "Tx", "V" => string("Asha Verma"), "Rect" => rect(250.0, 696.0, 450.0, 712.0),
            "P" => p1, "AP" => dictionary! { "N" => name_ap },
        });
        let pan = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Widget", "Parent" => applicant,
            "T" => string("panNumber"), "FT" => "Tx", "V" => string("ABCDE1234F"),
            "Rect" => rect(250.0, 666.0, 450.0, 682.0), "P" => p1,
        });
        doc.objects.insert(
            applicant,
            Object::Dictionary(dictionary! {
                "T" => string("applicant"),
                "Kids" => vec![name.into(), pan.into()],
            }),
        );
        annots1.extend([name.into(), pan.into()]);

        let resident_ap = states(&mut doc, "Choice1");
        let resident = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Widget", "T" => string("resident"),
            "TU" => string("Resident of India"), "FT" => "Btn", "V" => "Choice1", "AS" => "Choice1",
            "Opt" => vec![string("Resident")], "Rect" => rect(72.0, 630.0, 84.0, 642.0),
            "P" => p1, "AP" => resident_ap,
        });
        let agree_ap = states(&mut doc, "Yes");
        let agree = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Widget", "T" => string("agree"),
            "TU" => string("Declaration accepted"), "FT" => "Btn", "V" => "Off", "AS" => "Off",
            "Rect" => rect(100.0, 630.0, 112.0, 642.0), "P" => p1, "AP" => agree_ap,
        });
        annots1.extend([resident.into(), agree.into()]);

        let regime = doc.new_object_id();
        let old_ap = states(&mut doc, "0");
        let old = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Widget", "Parent" => regime, "AS" => "Off",
            "Rect" => rect(72.0, 580.0, 84.0, 592.0), "P" => p1, "AP" => old_ap,
        });
        let new_ap = states(&mut doc, "1");
        let new = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Widget", "Parent" => regime, "AS" => "1",
            "Rect" => rect(150.0, 580.0, 162.0, 592.0), "P" => p1, "AP" => new_ap,
        });
        doc.objects.insert(
            regime,
            Object::Dictionary(dictionary! {
                "T" => string("regime"), "TU" => string("Tax regime"), "FT" => "Btn",
                "Ff" => FF_RADIO, "V" => "1",
                "Opt" => vec![string("Old regime"), string("New regime")],
                "Kids" => vec![old.into(), new.into()],
            }),
        );
        annots1.extend([old.into(), new.into()]);

        let states_field = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Widget", "T" => string("states"),
            "TU" => string("States of income"), "FT" => "Ch", "Ff" => FF_MULTISELECT,
            "V" => vec![string("MH"), string("KA")],
            "Opt" => vec![
                Object::Array(vec![string("MH"), string("Maharashtra")]),
                Object::Array(vec![string("KA"), string("Karnataka")]),
                Object::Array(vec![string("DL"), string("Delhi")]),
            ],
            "Rect" => rect(72.0, 540.0, 300.0, 560.0), "P" => p1,
        });
        let submit = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Widget", "T" => string("submit"),
            "FT" => "Btn", "Ff" => FF_PUSHBUTTON, "Rect" => rect(400.0, 100.0, 480.0, 120.0), "P" => p1,
        });
        annots1.extend([states_field.into(), submit.into()]);

        // Page 2: no /P on the widget; its page comes from the page's /Annots.
        let employer = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Widget", "T" => string("employer"),
            "FT" => "Tx", "V" => string("Acme Tools Ltd"), "Rect" => rect(250.0, 736.0, 450.0, 752.0),
        });
        let empty = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Widget", "T" => string("notes"),
            "FT" => "Tx", "Rect" => rect(72.0, 600.0, 450.0, 640.0),
        });
        let comment = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Text", "Contents" => string("Check the totals"),
            "Rect" => rect(500.0, 700.0, 520.0, 720.0),
        });
        annots2.extend([employer.into(), empty.into(), comment.into()]);

        for (page, annots) in [(p1, annots1), (p2, annots2)] {
            if let Ok(Object::Dictionary(d)) = doc.get_object_mut(page) {
                d.set("Annots", annots);
            }
        }
        let fields: Vec<Object> = vec![
            applicant.into(),
            resident.into(),
            agree.into(),
            regime.into(),
            states_field.into(),
            submit.into(),
            employer.into(),
            empty.into(),
        ];
        let acroform = doc.add_object(dictionary! { "Fields" => fields });
        let catalog = doc
            .trailer
            .get(b"Root")
            .and_then(Object::as_reference)
            .expect("root");
        if let Ok(Object::Dictionary(d)) = doc.get_object_mut(catalog) {
            d.set("AcroForm", acroform);
        }
        let mut out = Vec::new();
        doc.save_to(&mut out).expect("save");
        out
    }

    fn field<'a>(forms: &'a FormExtraction, name: &str) -> &'a FormField {
        forms
            .fields
            .iter()
            .find(|f| f.name == name)
            .unwrap_or_else(|| panic!("no field {name}: {:#?}", forms.fields))
    }

    #[test]
    fn acroform_fields_are_read_with_names_labels_types_pages_and_boxes() {
        let forms = extract_forms(&form_pdf()).expect("forms");
        let name = field(&forms, "applicant.name[0]");
        assert_eq!(name.label, "Full name of the applicant");
        assert_eq!(
            (name.kind, name.value.as_str()),
            (FieldKind::Text, "Asha Verma")
        );
        assert_eq!(name.page, Some(1));
        assert_eq!(name.bbox, Some(BBox::new(250.0, 696.0, 450.0, 712.0)));
        // No tooltip: the readable partial name.
        assert_eq!(field(&forms, "applicant.panNumber").label, "pan Number");

        let resident = field(&forms, "resident");
        assert_eq!(
            (resident.kind, resident.value.as_str()),
            (FieldKind::Checkbox, "Yes (Resident)")
        );
        assert_eq!(field(&forms, "agree").value, "No");

        let regime = field(&forms, "regime");
        assert_eq!(
            (regime.kind, regime.value.as_str()),
            (FieldKind::Radio, "New regime")
        );
        // The selected widget's box.
        assert_eq!(regime.bbox, Some(BBox::new(150.0, 580.0, 162.0, 592.0)));

        let states = field(&forms, "states");
        assert_eq!(
            (states.kind, states.value.as_str()),
            (FieldKind::Choice, "Maharashtra, Karnataka")
        );

        assert!(
            forms.fields.iter().all(|f| f.name != "submit"),
            "push buttons carry no value"
        );
        let employer = field(&forms, "employer");
        assert_eq!(
            (employer.page, employer.value.as_str()),
            (Some(2), "Acme Tools Ltd")
        );
        assert_eq!(field(&forms, "notes").value, "");

        let comment = forms
            .fields
            .iter()
            .find(|f| f.kind == FieldKind::Comment)
            .expect("comment");
        assert_eq!(
            (comment.value.as_str(), comment.page),
            ("Check the totals", Some(2))
        );

        let stats = &forms.stats;
        assert!(stats.acroform);
        assert_eq!(stats.terminal_fields, 9);
        assert_eq!((stats.checkboxes, stats.radios, stats.choices), (2, 1, 1));
        assert_eq!(stats.comments, 1);
    }

    #[test]
    fn a_pdf_wrapped_after_leading_bytes_and_one_without_a_form_are_read() {
        let mut wrapped = b"\xac\xed\x00\x05ur\x00\x02[B".to_vec();
        wrapped.extend(form_pdf());
        let forms = extract_forms(&wrapped).expect("forms after leading bytes");
        assert_eq!(field(&forms, "employer").value, "Acme Tools Ltd");

        let plain = build_pdf(&[vec![text(72.0, 700.0, 10.0, "No form here.")]], None);
        let forms = extract_forms(&plain).expect("plain pdf");
        assert!(forms.fields.is_empty());
        assert!(!forms.stats.acroform);
        assert!(matches!(
            extract_forms(b"not a pdf"),
            Err(FormError::Open(_))
        ));
    }

    #[test]
    fn field_names_become_readable_labels() {
        assert_eq!(
            readable_name("form1[0].page1[0].grossSalary[0]"),
            "gross Salary"
        );
        assert_eq!(readable_name("tax_paid"), "tax paid");
        assert_eq!(
            pdf_date("D:20240131120000+05'30'").as_deref(),
            Some("2024-01-31")
        );
        assert_eq!(name_text(b"New#20regime"), "New regime");
    }
}
