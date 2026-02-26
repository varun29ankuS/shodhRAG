//! Smart Templates System
//!
//! Extracts document structures and generates reusable templates from existing documents.
//! Learns formatting, section patterns, and content organization.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::LazyLock;
use crate::types::ComprehensiveResult;
use crate::rag_engine::RAGEngine;

static NUMBERED_HEADING_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^\d+\.\s+(.+)$").expect("numbered heading regex is valid")
});
static NUMBER_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"\b\d+\b").expect("number regex is valid")
});
static DATE_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"\d{4}-\d{2}-\d{2}").expect("date regex is valid")
});
static NAME_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"\b[A-Z][a-z]+ [A-Z][a-z]+\b").expect("name regex is valid")
});
static MUSTACHE_VAR_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"\{\{(\w+)\}\}").expect("mustache var regex is valid")
});
static BRACKET_VAR_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"\[([A-Z_]+)\]").expect("bracket var regex is valid")
});

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentTemplate {
    pub id: String,
    pub name: String,
    pub description: String,
    pub sections: Vec<TemplateSection>,
    pub metadata: TemplateMetadata,
    pub variables: Vec<TemplateVariable>,
    pub example_content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateSection {
    pub name: String,
    pub order: usize,
    pub content_type: ContentType,
    pub placeholder: String,
    pub is_required: bool,
    pub formatting_rules: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ContentType {
    Text,
    List,
    Table,
    Code,
    Quote,
    Heading,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateMetadata {
    pub document_type: String,
    pub industry: Option<String>,
    pub language: String,
    pub created_from: Vec<String>,
    pub confidence_score: f32,
    pub usage_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateVariable {
    pub name: String,
    pub description: String,
    pub default_value: Option<String>,
    pub validation_pattern: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateExtractionRequest {
    pub document_ids: Vec<String>,
    pub template_name: String,
    pub auto_detect_sections: bool,
    pub preserve_formatting: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateGenerationRequest {
    pub template_id: String,
    pub variables: HashMap<String, String>,
    pub output_format: OutputFormat,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OutputFormat {
    Markdown,
    PlainText,
    Html,
    Json,
}

pub struct TemplateExtractor;

impl TemplateExtractor {
    pub fn new() -> Self {
        Self
    }

    /// Extract template from multiple documents by analyzing common patterns
    pub async fn extract_template(
        &self,
        request: TemplateExtractionRequest,
        rag: &RAGEngine,
    ) -> Result<DocumentTemplate, String> {
        let mut all_chunks = Vec::new();
        for doc_id in &request.document_ids {
            let chunks = self.get_document_chunks(doc_id, rag).await?;
            all_chunks.extend(chunks);
        }

        if all_chunks.is_empty() {
            return Err("No content found in specified documents".to_string());
        }

        let sections = if request.auto_detect_sections {
            self.detect_sections(&all_chunks)?
        } else {
            self.extract_manual_sections(&all_chunks)?
        };

        let formatting_rules = self.detect_formatting_patterns(&all_chunks);
        let variables = self.extract_variables(&all_chunks);
        let metadata = self.analyze_metadata(&all_chunks, &request.document_ids);
        let example_content = self.generate_example(&sections, &all_chunks);

        let template = DocumentTemplate {
            id: uuid::Uuid::new_v4().to_string(),
            name: request.template_name,
            description: format!(
                "Template extracted from {} documents with {} sections",
                request.document_ids.len(),
                sections.len()
            ),
            sections,
            metadata,
            variables,
            example_content,
        };

        Ok(template)
    }

    /// Generate new document from template
    pub fn generate_from_template(
        &self,
        request: TemplateGenerationRequest,
        templates: &HashMap<String, DocumentTemplate>,
    ) -> Result<String, String> {
        let template = templates
            .get(&request.template_id)
            .ok_or_else(|| "Template not found".to_string())?;

        let mut output = String::new();

        for section in &template.sections {
            match request.output_format {
                OutputFormat::Markdown => {
                    output.push_str(&format!("## {}\n\n", section.name));
                }
                OutputFormat::Html => {
                    output.push_str(&format!("<h2>{}</h2>\n", section.name));
                }
                _ => {
                    output.push_str(&format!("{}\n\n", section.name));
                }
            }

            let mut content = section.placeholder.clone();
            for (var_name, var_value) in &request.variables {
                let patterns = vec![
                    format!("{{{{{}}}}}", var_name),
                    format!("[{}]", var_name.to_uppercase()),
                    format!("${}", var_name),
                ];
                for pattern in patterns {
                    content = content.replace(&pattern, var_value);
                }
            }

            output.push_str(&content);
            output.push_str("\n\n");
        }

        Ok(output)
    }

    async fn get_document_chunks(
        &self,
        doc_id: &str,
        rag: &RAGEngine,
    ) -> Result<Vec<ComprehensiveResult>, String> {
        let results = rag
            .list_documents(None, 10000)
            .await
            .map_err(|e| format!("Failed to list documents: {}", e))?;

        let chunks: Vec<_> = results
            .into_iter()
            .filter(|r| {
                r.metadata
                    .get("file_path")
                    .or_else(|| r.metadata.get("document_id"))
                    .map(|id| id == doc_id || id.contains(doc_id))
                    .unwrap_or(false)
            })
            .collect();

        Ok(chunks)
    }

    fn detect_sections(
        &self,
        chunks: &[ComprehensiveResult],
    ) -> Result<Vec<TemplateSection>, String> {
        let mut sections = Vec::new();
        let mut section_map: HashMap<String, Vec<String>> = HashMap::new();

        for chunk in chunks {
            let content = &chunk.snippet;
            if let Some(heading) = self.extract_heading(content) {
                section_map
                    .entry(heading.clone())
                    .or_default()
                    .push(content.clone());
            }
        }

        let mut order = 0;
        for (section_name, contents) in section_map {
            let content_type = self.infer_content_type(&contents);
            let placeholder = self.create_placeholder(&section_name, &contents);

            sections.push(TemplateSection {
                name: section_name.clone(),
                order,
                content_type,
                placeholder,
                is_required: true,
                formatting_rules: vec![],
            });
            order += 1;
        }

        sections.sort_by_key(|s| s.order);

        if sections.is_empty() {
            sections = self.create_default_sections(chunks);
        }

        Ok(sections)
    }

    fn extract_manual_sections(
        &self,
        chunks: &[ComprehensiveResult],
    ) -> Result<Vec<TemplateSection>, String> {
        let mut sections = Vec::new();

        for (idx, chunk) in chunks.iter().enumerate() {
            let section_name = chunk
                .metadata
                .get("section")
                .cloned()
                .unwrap_or_else(|| format!("Section {}", idx + 1));

            sections.push(TemplateSection {
                name: section_name,
                order: idx,
                content_type: ContentType::Text,
                placeholder: chunk.snippet.clone(),
                is_required: true,
                formatting_rules: vec![],
            });
        }

        Ok(sections)
    }

    fn extract_heading(&self, content: &str) -> Option<String> {
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("##") {
                return Some(trimmed.trim_start_matches('#').trim().to_string());
            }
            if trimmed.starts_with("# ") {
                return Some(trimmed.trim_start_matches('#').trim().to_string());
            }
            if let Some(captures) = NUMBERED_HEADING_RE.captures(trimmed) {
                return captures.get(1).map(|m| m.as_str().to_string());
            }
        }
        None
    }

    fn infer_content_type(&self, contents: &[String]) -> ContentType {
        let combined = contents.join("\n");

        if combined.contains("```") || combined.contains("fn ") || combined.contains("class ") {
            ContentType::Code
        } else if combined.contains("- ") || combined.contains("* ") {
            ContentType::List
        } else if combined.contains("|") && combined.matches('|').count() > 4 {
            ContentType::Table
        } else if combined.starts_with('>') {
            ContentType::Quote
        } else if contents.len() == 1 && contents[0].lines().count() <= 2 {
            ContentType::Heading
        } else {
            ContentType::Text
        }
    }

    fn create_placeholder(&self, section_name: &str, contents: &[String]) -> String {
        let example = contents.first().cloned().unwrap_or_default();

        let mut placeholder = example.clone();

        placeholder = DATE_RE.replace_all(&placeholder, "[DATE]").to_string();
        placeholder = NAME_RE.replace_all(&placeholder, "[NAME]").to_string();
        placeholder = NUMBER_RE.replace_all(&placeholder, "[NUMBER]").to_string();

        if placeholder.len() > 500 {
            format!("[{}]\n\nProvide content for this section.", section_name.to_uppercase())
        } else {
            placeholder
        }
    }

    fn create_default_sections(
        &self,
        _chunks: &[ComprehensiveResult],
    ) -> Vec<TemplateSection> {
        vec![
            TemplateSection {
                name: "Introduction".to_string(),
                order: 0,
                content_type: ContentType::Text,
                placeholder: "[INTRODUCTION]\n\nProvide an introduction to the document."
                    .to_string(),
                is_required: true,
                formatting_rules: vec![],
            },
            TemplateSection {
                name: "Main Content".to_string(),
                order: 1,
                content_type: ContentType::Text,
                placeholder: "[MAIN_CONTENT]\n\nProvide the main content of the document."
                    .to_string(),
                is_required: true,
                formatting_rules: vec![],
            },
            TemplateSection {
                name: "Conclusion".to_string(),
                order: 2,
                content_type: ContentType::Text,
                placeholder: "[CONCLUSION]\n\nProvide a conclusion for the document."
                    .to_string(),
                is_required: false,
                formatting_rules: vec![],
            },
        ]
    }

    fn detect_formatting_patterns(&self, chunks: &[ComprehensiveResult]) -> Vec<String> {
        let mut patterns = Vec::new();

        let combined = chunks.iter().map(|c| c.snippet.as_str()).collect::<Vec<_>>().join("\n");

        if combined.contains("**") || combined.contains("__") {
            patterns.push("Uses bold text for emphasis".to_string());
        }
        if combined.contains("- [ ]") || combined.contains("- [x]") {
            patterns.push("Uses task lists".to_string());
        }
        if combined.contains("```") {
            patterns.push("Includes code blocks".to_string());
        }
        if combined.contains("[^") {
            patterns.push("Uses footnotes".to_string());
        }

        patterns
    }

    fn extract_variables(&self, chunks: &[ComprehensiveResult]) -> Vec<TemplateVariable> {
        let mut variables = Vec::new();
        let combined = chunks.iter().map(|c| c.snippet.as_str()).collect::<Vec<_>>().join("\n");

        for cap in MUSTACHE_VAR_RE.captures_iter(&combined) {
            if let Some(var_name) = cap.get(1) {
                variables.push(TemplateVariable {
                    name: var_name.as_str().to_string(),
                    description: format!("Variable: {}", var_name.as_str()),
                    default_value: None,
                    validation_pattern: None,
                });
            }
        }

        for cap in BRACKET_VAR_RE.captures_iter(&combined) {
            if let Some(var_name) = cap.get(1) {
                let name = var_name.as_str().to_lowercase();
                if !variables.iter().any(|v| v.name == name) {
                    variables.push(TemplateVariable {
                        name,
                        description: format!("Placeholder: {}", var_name.as_str()),
                        default_value: None,
                        validation_pattern: None,
                    });
                }
            }
        }

        variables
    }

    fn analyze_metadata(
        &self,
        chunks: &[ComprehensiveResult],
        doc_ids: &[String],
    ) -> TemplateMetadata {
        let combined = chunks.iter().map(|c| c.snippet.as_str()).collect::<Vec<_>>().join("\n");

        let document_type = if combined.contains("Agreement") || combined.contains("Contract") {
            "Legal Document"
        } else if combined.contains("Report") || combined.contains("Analysis") {
            "Report"
        } else if combined.contains("function") || combined.contains("class") {
            "Technical Documentation"
        } else {
            "General Document"
        }
        .to_string();

        TemplateMetadata {
            document_type,
            industry: None,
            language: "en".to_string(),
            created_from: doc_ids.to_vec(),
            confidence_score: 0.85,
            usage_count: 0,
        }
    }

    fn generate_example(&self, sections: &[TemplateSection], chunks: &[ComprehensiveResult]) -> String {
        let mut example = String::new();

        for section in sections {
            example.push_str(&format!("## {}\n\n", section.name));

            if let Some(chunk) = chunks.first() {
                let snippet = chunk.snippet.lines().take(3).collect::<Vec<_>>().join("\n");
                example.push_str(&snippet);
                example.push_str("\n\n");
            } else {
                example.push_str(&section.placeholder);
                example.push_str("\n\n");
            }
        }

        example
    }
}

impl Default for TemplateExtractor {
    fn default() -> Self {
        Self::new()
    }
}
