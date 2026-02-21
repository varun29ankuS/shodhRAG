//! Tauri commands for Smart Templates

use crate::smart_templates::{
    DocumentTemplate, OutputFormat, TemplateExtractor, TemplateExtractionRequest,
    TemplateGenerationRequest,
};
use crate::rag_commands::RagState;
use std::collections::HashMap;
use std::sync::Mutex;
use tauri::State;

// Global template storage
pub struct TemplateStore {
    pub templates: Mutex<HashMap<String, DocumentTemplate>>,
}

impl Default for TemplateStore {
    fn default() -> Self {
        Self {
            templates: Mutex::new(HashMap::new()),
        }
    }
}

/// Extract template from multiple documents
#[tauri::command]
pub async fn extract_template(
    document_ids: Vec<String>,
    template_name: String,
    auto_detect_sections: bool,
    rag_state: State<'_, RagState>,
    template_store: State<'_, TemplateStore>,
) -> Result<DocumentTemplate, String> {
    tracing::info!("\n=== EXTRACT TEMPLATE ===");
    tracing::info!("Documents: {:?}", document_ids);
    tracing::info!("Template name: {}", template_name);
    tracing::info!("Auto-detect sections: {}", auto_detect_sections);

    if document_ids.is_empty() {
        return Err("Need at least 1 document to extract template".to_string());
    }

    let request = TemplateExtractionRequest {
        document_ids,
        template_name,
        auto_detect_sections,
        preserve_formatting: true,
    };

    // Get RAG instance
    let rag_guard = rag_state.rag.read().await;
    let rag = &*rag_guard;

    // Extract template
    let extractor = TemplateExtractor::new();
    let template = extractor
        .extract_template(request, rag)
        .await
        .map_err(|e| format!("Template extraction failed: {}", e))?;

    // Store template
    let mut templates = template_store.templates.lock().map_err(|e| e.to_string())?;
    templates.insert(template.id.clone(), template.clone());

    tracing::info!("Template extracted successfully!");
    tracing::info!("Template ID: {}", template.id);
    tracing::info!("Sections: {}", template.sections.len());
    tracing::info!("Variables: {}", template.variables.len());

    Ok(template)
}

/// Generate document from template
#[tauri::command]
pub async fn generate_from_template(
    template_id: String,
    variables: HashMap<String, String>,
    output_format: String,
    template_store: State<'_, TemplateStore>,
) -> Result<String, String> {
    tracing::info!("\n=== GENERATE FROM TEMPLATE ===");
    tracing::info!("Template ID: {}", template_id);
    tracing::info!("Variables: {:?}", variables);
    tracing::info!("Output format: {}", output_format);

    // Parse output format
    let format = match output_format.to_lowercase().as_str() {
        "markdown" | "md" => OutputFormat::Markdown,
        "html" => OutputFormat::Html,
        "json" => OutputFormat::Json,
        "text" | "txt" => OutputFormat::PlainText,
        _ => return Err(format!("Invalid output format: {}", output_format)),
    };

    let request = TemplateGenerationRequest {
        template_id: template_id.clone(),
        variables,
        output_format: format,
    };

    // Get templates
    let templates = template_store.templates.lock().map_err(|e| e.to_string())?;

    // Generate document
    let extractor = TemplateExtractor::new();
    let output = extractor
        .generate_from_template(request, &templates)
        .map_err(|e| format!("Template generation failed: {}", e))?;

    tracing::info!("Document generated successfully!");
    tracing::info!("Output length: {} characters", output.len());

    Ok(output)
}

/// List all available templates
#[tauri::command]
pub async fn list_templates(
    template_store: State<'_, TemplateStore>,
) -> Result<Vec<DocumentTemplate>, String> {
    tracing::info!("Listing all templates...");

    let templates = template_store.templates.lock().map_err(|e| e.to_string())?;

    let template_list: Vec<DocumentTemplate> = templates.values().cloned().collect();

    tracing::info!("Found {} templates", template_list.len());
    Ok(template_list)
}

/// Get specific template by ID
#[tauri::command]
pub async fn get_template(
    template_id: String,
    template_store: State<'_, TemplateStore>,
) -> Result<DocumentTemplate, String> {
    tracing::info!("Getting template: {}", template_id);

    let templates = template_store.templates.lock().map_err(|e| e.to_string())?;

    let template = templates
        .get(&template_id)
        .ok_or_else(|| "Template not found".to_string())?;

    Ok(template.clone())
}

/// Delete template
#[tauri::command]
pub async fn delete_template(
    template_id: String,
    template_store: State<'_, TemplateStore>,
) -> Result<(), String> {
    tracing::info!("Deleting template: {}", template_id);

    let mut templates = template_store.templates.lock().map_err(|e| e.to_string())?;

    templates
        .remove(&template_id)
        .ok_or_else(|| "Template not found".to_string())?;

    tracing::info!("Template deleted successfully");
    Ok(())
}

/// Update template metadata
#[tauri::command]
pub async fn update_template(
    template_id: String,
    name: Option<String>,
    description: Option<String>,
    template_store: State<'_, TemplateStore>,
) -> Result<DocumentTemplate, String> {
    tracing::info!("Updating template: {}", template_id);

    let mut templates = template_store.templates.lock().map_err(|e| e.to_string())?;

    let template = templates
        .get_mut(&template_id)
        .ok_or_else(|| "Template not found".to_string())?;

    if let Some(new_name) = name {
        template.name = new_name;
    }

    if let Some(new_description) = description {
        template.description = new_description;
    }

    tracing::info!("Template updated successfully");
    Ok(template.clone())
}

/// Preview template with sample data
#[tauri::command]
pub async fn preview_template(
    template_id: String,
    template_store: State<'_, TemplateStore>,
) -> Result<String, String> {
    tracing::info!("Previewing template: {}", template_id);

    let templates = template_store.templates.lock().map_err(|e| e.to_string())?;

    let template = templates
        .get(&template_id)
        .ok_or_else(|| "Template not found".to_string())?;

    // Use example content as preview
    Ok(template.example_content.clone())
}
