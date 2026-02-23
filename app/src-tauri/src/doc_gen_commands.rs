use serde::{Deserialize, Serialize};
use tauri::{State, Emitter};
use crate::rag_commands::RagState;
use crate::llm_commands::LLMState;
use shodh_rag::comprehensive_system::SimpleSearchResult;
use shodh_rag::llm::{LLMMode, ApiProvider};
use uuid::Uuid;
use printpdf::*;
use std::fs::File;
use std::io::BufWriter;
use docx_rs::*;
use rust_xlsxwriter::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DocumentLength {
    Brief,       // ~2k tokens (1-2 pages)
    Standard,    // ~8k tokens (4-8 pages)
    Detailed,    // ~32k tokens (16-32 pages)
    Maximum,     // Provider max (could be 100k+)
}

impl DocumentLength {
    fn to_tokens(&self) -> usize {
        match self {
            DocumentLength::Brief => 2_048,
            DocumentLength::Standard => 8_192,
            DocumentLength::Detailed => 32_768,
            DocumentLength::Maximum => 128_000, // Will be capped by provider
        }
    }

    fn description(&self) -> &str {
        match self {
            DocumentLength::Brief => "1-2 pages, quick summary",
            DocumentLength::Standard => "4-8 pages, standard report",
            DocumentLength::Detailed => "16-32 pages, comprehensive analysis",
            DocumentLength::Maximum => "Maximum length, full deep-dive",
        }
    }
}

/// Get maximum tokens supported by the current LLM provider
fn get_provider_max_tokens(llm_mode: &LLMMode) -> usize {
    match llm_mode {
        LLMMode::External { provider, .. } => match provider {
            ApiProvider::Anthropic => 100_000,    // Claude Opus/Sonnet can generate huge outputs
            ApiProvider::OpenRouter => 32_768,    // Most OpenRouter models support this
            ApiProvider::OpenAI => 16_384,        // GPT-4 Turbo max output
            ApiProvider::Together => 8_192,       // Together AI typical max
            ApiProvider::Grok => 8_192,           // Grok default
            ApiProvider::Perplexity => 4_096,     // Perplexity default
            ApiProvider::Google => 8_192,         // Gemini 2.5 Pro max output tokens
            ApiProvider::Replicate => 8_192,      // Replicate typical max
            ApiProvider::Baseten => 16_384,       // GPT-OSS-120B supports 16K output
            ApiProvider::Ollama => 8_192,               // Ollama, depends on model
            ApiProvider::HuggingFace { .. } => 4_096,  // HuggingFace varies by model
            ApiProvider::Custom { .. } => 8_192,  // Custom endpoints, conservative default
        },
        LLMMode::Local { .. } => 4_096,  // Local ONNX models are architecturally limited
        LLMMode::Disabled => 4_096,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerateDocumentRequest {
    pub format: String,
    pub template_id: Option<String>,
    pub data: serde_json::Value,
    pub query: Option<String>,
    pub use_rag: bool,
    pub desired_length: Option<DocumentLength>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerateDocumentResponse {
    pub id: String,
    pub title: String,
    pub format: String,
    pub size: usize,
    pub pages: Option<usize>,
    pub preview: Option<String>,
    #[serde(rename = "contentBase64")]
    pub content_base64: Option<String>,
    pub metadata: DocumentMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentMetadata {
    #[serde(rename = "createdAt")]
    pub created_at: String,
    pub sources: Vec<String>,
    pub template: Option<String>,
    #[serde(rename = "generationTimeMs")]
    pub generation_time_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub format: String,
}

/// Generate a document with RAG and LLM integration
#[tauri::command]
pub async fn generate_document(
    request: GenerateDocumentRequest,
    rag_state: State<'_, RagState>,
    llm_state: State<'_, LLMState>,
) -> Result<GenerateDocumentResponse, String> {
    tracing::info!("Generating document with format: {}", request.format);
    
    // Generate content based on format
    let content = match request.format.as_str() {
        "html" => generate_html_report(request.clone(), rag_state.clone(), llm_state.clone()).await?,
        "pdf" => {
            let pdf_bytes = generate_pdf_report(request.clone(), rag_state.clone(), llm_state.clone()).await?;
            // Return base64 encoded PDF
            base64::encode(pdf_bytes)
        },
        "txt" => generate_text_report(request.clone(), rag_state.clone(), llm_state.clone()).await?,
        "docx" => {
            let docx_bytes = generate_docx_content(request.clone(), rag_state.clone(), llm_state.clone()).await?;
            // Return base64 encoded DOCX
            base64::encode(docx_bytes)
        },
        "xlsx" | "csv" => {
            let xlsx_bytes = generate_spreadsheet_content(request.clone(), rag_state.clone(), llm_state.clone()).await?;
            // Return base64 encoded Excel
            base64::encode(xlsx_bytes)
        },
        "json" => generate_json_report(request.clone(), rag_state.clone(), llm_state.clone()).await?,
        "md" | _ => {
            // Generate markdown format
            if request.use_rag && request.query.is_some() {
        let query = request.query.as_deref().unwrap_or("");
        
        // First, perform RAG search with more results for comprehensive generation
        let search_results = {
            let rag_guard = rag_state.rag.read().await;
            let rag = &*rag_guard;
            match rag.search(query, 15).await {  // Get more results for better context
                    Ok(mut results) => {
                        // Sort by relevance
                        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

                        // Log what we found for debugging
                        tracing::info!("📚 Found {} documents for generation query: {}", results.len(), query);
                        for (i, result) in results.iter().enumerate().take(5) {
                            if let Some(title) = result.metadata.get("title") {
                                tracing::info!("  {}. {} (score: {:.3})", i+1, title, result.score);
                            }
                        }

                        results
                    },
                    Err(e) => {
                        tracing::warn!("RAG search failed: {}", e);
                        vec![]
                    }
                }
        };

        // Extract context with metadata for richer document generation
        let context: Vec<String> = search_results.iter()
            .map(|r| {
                let mut context_item = String::new();

                // Add file metadata if available
                if let Some(filename) = r.metadata.get("filename").or(r.metadata.get("title")) {
                    context_item.push_str(&format!("[Source: {}]\n", filename));
                }

                // Add file type for code awareness
                if let Some(ext) = r.metadata.get("file_extension") {
                    let file_type = match ext.as_str() {
                        "rs" => "Rust Code",
                        "py" => "Python Code",
                        "js" | "ts" => "JavaScript/TypeScript",
                        "md" => "Markdown Documentation",
                        "json" => "JSON Data",
                        _ => "Text"
                    };
                    context_item.push_str(&format!("[Type: {}]\n", file_type));
                }

                // Add the actual content
                context_item.push_str("---\n");
                context_item.push_str(&r.text);
                context_item.push_str("\n---\n");

                context_item
            })
            .collect();

        // Generate content using LLM (with or without RAG context)
        let llm_content = {
            let manager_lock = llm_state.manager.read().await;
            if let Some(manager) = manager_lock.as_ref() {
                // Check if we have sources or generating directly from LLM knowledge
                let has_sources = !context.is_empty();

                // Analyze the context to understand what type of content we have (if any)
                let has_code = has_sources && context.iter().any(|c|
                    c.contains("[Type: Rust Code]") ||
                    c.contains("[Type: Python Code]") ||
                    c.contains("[Type: JavaScript/TypeScript]")
                );

                let has_docs = has_sources && context.iter().any(|c|
                    c.contains("[Type: Markdown Documentation]")
                );

                // Refuse generation without sources — prevents hallucination
                if !has_sources {
                    return Err(
                        "Cannot generate document: No relevant information found in the knowledge base \
                        for this query. Please add documents to your knowledge base first, then try again."
                            .to_string(),
                    );
                }

                // Grounding + formatting preamble shared by every prompt variant
                let grounding_rules = "\
GROUNDING RULES (non-negotiable):
- You MUST use ONLY the Source Materials below. You have NO other knowledge.
- For EVERY claim, cite the source with [Source N] inline where N is the source number.
- If a fact is not explicitly stated in the Source Materials, DO NOT include it.
- NEVER infer, assume, or extrapolate beyond what the sources explicitly state.
- An incomplete but 100% accurate document is better than a comprehensive but partially wrong one.
- If the sources contain insufficient information for a section, write: \"Insufficient data in indexed documents.\"

FORMATTING RULES (produce a pristine, publication-ready document):
- Use clean Markdown: # for title, ## for major sections, ### for subsections.
- NO emojis anywhere in the document.
- Use proper paragraph spacing — one blank line between paragraphs.
- Use **bold** for key terms and emphasis, not for entire sentences.
- Use bullet points or numbered lists for enumerations — not run-on paragraphs.
- Tables: use Markdown pipe tables for any structured/comparative data.
- Keep language concise, professional, and formal.
- End with a ## References section listing every source used (numbered to match inline citations).\n";

                // Build the prompt based on content type
                let doc_prompt = if has_code && (query.contains("implement") || query.contains("code") || query.contains("function") || query.contains("class")) {
                    format!(
                        "Create a professional technical document.\n\n\
                        Topic: {query}\n\n\
                        {grounding_rules}\n\
                        Document structure:\n\
                        ## Overview\n\
                        Brief description of what the codebase covers.\n\n\
                        ## Architecture\n\
                        System design and component relationships found in the sources.\n\n\
                        ## Implementation Details\n\
                        Key functions, classes, and modules with code blocks (```language).\n\n\
                        ## Usage Examples\n\
                        Practical examples derived from the sources.\n\n\
                        ## References\n\
                        List all sources used.\n\n\
                        Source Materials:\n{context}",
                        query = query,
                        grounding_rules = grounding_rules,
                        context = context.join("\n\n"),
                    )
                } else if query.contains("report") || query.contains("analysis") || query.contains("summary") {
                    format!(
                        "Create a professional report.\n\n\
                        Topic: {query}\n\n\
                        {grounding_rules}\n\
                        Document structure:\n\
                        ## Executive Summary\n\
                        2-3 paragraph overview of key findings.\n\n\
                        ## Background\n\
                        Context and scope as stated in the sources.\n\n\
                        ## Detailed Findings\n\
                        Evidence and data with [Source N] citations. Use tables where data is comparative.\n\n\
                        ## Analysis\n\
                        Interpretation strictly based on source evidence.\n\n\
                        ## Recommendations\n\
                        Actionable next steps supported by findings.\n\n\
                        ## References\n\
                        Numbered list of all sources used.\n\n\
                        Source Materials:\n{context}",
                        query = query,
                        grounding_rules = grounding_rules,
                        context = context.join("\n\n"),
                    )
                } else {
                    format!(
                        "Create a professional document.\n\n\
                        Topic: {query}\n\n\
                        {grounding_rules}\n\
                        Document structure:\n\
                        ## Introduction\n\
                        Brief overview of what the sources cover and scope of this document.\n\n\
                        ## [Organize remaining content into logical sections based on the source material]\n\
                        Use ## for major sections, ### for subsections. Include [Source N] citations.\n\n\
                        ## Conclusion\n\
                        Key takeaways from the sources.\n\n\
                        ## References\n\
                        Numbered list of all sources used.\n\n\
                        Source Materials:\n{context}",
                        query = query,
                        grounding_rules = grounding_rules,
                        context = context.join("\n\n"),
                    )
                };

                // Get max_tokens from request data, default to 8192
                let max_tokens = request.data.get("max_tokens")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as usize)
                    .unwrap_or(8_192);

                tracing::info!("🎯 Using max_tokens: {} for document generation", max_tokens);

                // Generate with RAG context (sources are guaranteed present at this point)
                tracing::info!("📚 Generating with {} sources", context.len());
                match manager.generate_with_rag_custom(&doc_prompt, context.clone(), max_tokens).await {
                    Ok(generated) => generated,
                    Err(e) => {
                        tracing::warn!("LLM generation failed, using context summary: {}", e);
                        format!("# {} Report\n\n## Query\n{}\n\n## Found Information\n\n{}",
                            query,
                            query,
                            context.join("\n\n---\n\n")
                        )
                    }
                }
            } else {
                // LLM not configured
                if !context.is_empty() {
                    // Have sources but no LLM - show sources
                    format!("# {} Report\n\n## Query\n{}\n\n## Found Information\n\n{}",
                        query,
                        query,
                        context.join("\n\n---\n\n")
                    )
                } else {
                    // No sources and no LLM - return error
                    return Err("Cannot generate document: LLM is not configured. Please configure an LLM provider to generate documents.".to_string());
                }
            }
        };
        
        // The LLM output already contains proper section structure and a References
        // section (enforced by the prompt). Just prepend a clean header with metadata.
        format!(
            "# {}\n\n**Date:** {}  \n**Query:** {}\n\n---\n\n{}",
            query,
            chrono::Utc::now().format("%B %d, %Y"),
            query,
            llm_content.trim(),
        )
            } else {
                format!(
                    "# Generated Document\n\n## Content\n{}",
                    serde_json::to_string_pretty(&request.data).unwrap_or_default()
                )
            }
        }
    };
    
    // Generate title
    let title = if let Some(query) = &request.query {
        format!("{}_report.{}", 
            query.chars()
                .filter(|c| c.is_alphanumeric() || c.is_whitespace())
                .collect::<String>()
                .replace(' ', "_")
                .to_lowercase(),
            request.format
        )
    } else {
        format!("document_{}.{}", 
            chrono::Utc::now().timestamp(),
            request.format
        )
    };
    
    // Extract source IDs for metadata
    let source_ids = if request.use_rag && request.query.is_some() {
        // We need to get the search results again to extract source IDs
        let query = request.query.as_deref().unwrap_or("");
        let rag_guard = rag_state.rag.read().await;
        let rag = &*rag_guard;
        match rag.search(query, 5).await {
            Ok(results) => results.iter().map(|r| r.id.to_string()).collect(),
            Err(_) => vec![],
        }
    } else {
        vec![]
    };
    
    // Create response with full content preview (no artificial limit)
    let response = GenerateDocumentResponse {
        id: uuid::Uuid::new_v4().to_string(),
        title,
        format: request.format.clone(),
        size: content.len(),
        pages: Some((content.len() / 2000).max(1)),  // Estimate ~2000 chars per page
        preview: Some(content.clone()),  // Full content, no truncation
        content_base64: Some(base64::encode(content.as_bytes())),
        metadata: DocumentMetadata {
            created_at: chrono::Utc::now().to_rfc3339(),
            sources: source_ids,
            template: request.template_id,
            generation_time_ms: 100,
        },
    };
    
    Ok(response)
}

/// Generate document from RAG search using the integrated doc-gen system
#[tauri::command]
pub async fn generate_from_rag(
    prompt: String,
    format: String,
    include_references: Option<bool>,
    max_source_docs: Option<usize>,
    template: Option<String>,
    desired_length: Option<DocumentLength>,
    rag_state: State<'_, RagState>,
    llm_state: State<'_, LLMState>,
) -> Result<GenerateDocumentResponse, String> {
    // Get current LLM mode to determine provider limits
    let llm_mode = {
        let config_guard = llm_state.config.lock().unwrap_or_else(|e| e.into_inner());
        config_guard.mode.clone()
    };

    let length = desired_length.unwrap_or(DocumentLength::Standard);
    let requested_tokens = length.to_tokens();
    let provider_max = get_provider_max_tokens(&llm_mode);
    let actual_max = requested_tokens.min(provider_max);

    tracing::info!("📄 Generating {} document for prompt: {}", format, prompt);
    tracing::info!("   Length: {:?} (requested: {}k, provider max: {}k, using: {}k tokens)",
             length, requested_tokens/1000, provider_max/1000, actual_max/1000);

    if requested_tokens > provider_max {
        tracing::info!("⚠️  Requested length exceeds provider capability, capping at {}k tokens", provider_max/1000);
    }

    let include_refs = include_references.unwrap_or(true);
    let max_docs = max_source_docs.unwrap_or(10);

    let request = GenerateDocumentRequest {
        format,
        template_id: template,
        data: serde_json::json!({
            "query": prompt,
            "include_references": include_refs,
            "max_source_docs": max_docs,
            "max_tokens": actual_max
        }),
        query: Some(prompt),
        use_rag: true,
        desired_length: Some(length),
    };

    generate_document(request, rag_state, llm_state).await
}

/// Get available document formats
#[tauri::command]
pub async fn get_available_formats() -> Result<Vec<String>, String> {
    Ok(vec![
        "pdf".to_string(),
        "docx".to_string(),
        "xlsx".to_string(),
        "md".to_string(),
        "txt".to_string(),
        "html".to_string(),
        "json".to_string(),
    ])
}

/// Get available templates
#[tauri::command]
pub async fn get_available_templates() -> Result<Vec<TemplateInfo>, String> {
    Ok(vec![
        TemplateInfo {
            id: "compliance_report".to_string(),
            name: "Compliance Report".to_string(),
            description: "Quarterly compliance report template".to_string(),
            format: "pdf".to_string(),
        },
        TemplateInfo {
            id: "executive_summary".to_string(),
            name: "Executive Summary".to_string(),
            description: "High-level summary for executives".to_string(),
            format: "docx".to_string(),
        },
        TemplateInfo {
            id: "data_export".to_string(),
            name: "Data Export".to_string(),
            description: "Export search results to spreadsheet".to_string(),
            format: "xlsx".to_string(),
        },
    ])
}

/// Generate preview for a document
#[tauri::command]
pub async fn generate_document_preview(
    document_id: String,
    format: String,
) -> Result<String, String> {
    Ok(format!(
        "<div style='padding: 20px;'><h2>Document Preview</h2><p>Format: {}</p><p>ID: {}</p></div>",
        format, document_id
    ))
}

/// Get source documents with actual metadata from search results
#[tauri::command]
pub async fn get_source_documents(
    document_ids: Vec<String>,
    rag_state: State<'_, RagState>,
) -> Result<Vec<serde_json::Value>, String> {
    let mut source_docs = Vec::new();
    let mut seen_docs = std::collections::HashSet::new();
    
    // We need to get the actual document metadata from the RAG system
    let rag_guard = rag_state.rag.read().await;
    let rag = &*rag_guard;

    // Fetch actual document metadata for each ID
        for id_str in document_ids.iter() {
            // Skip duplicates
            if !seen_docs.insert(id_str.clone()) {
                continue;
            }
            
            // Parse the UUID if possible
            if let Ok(doc_id) = Uuid::parse_str(id_str) {
                // Get document statistics to find metadata
                let _stats = rag.get_statistics().await.unwrap_or_default();
                
                // Since we don't have direct document lookup, we search for it
                // This is a production workaround until we implement proper document storage
                let search_results = rag.search(id_str, 1).await.map_err(|e| e.to_string())?;
                
                if let Some(result) = search_results.first() {
                    // Extract actual metadata from the search result
                    let doc_type = result.metadata.get("format")
                        .or_else(|| result.metadata.get("type"))
                        .cloned()
                        .unwrap_or_else(|| {
                            if result.source.ends_with(".pdf") { "pdf".to_string() }
                            else if result.source.ends_with(".docx") { "docx".to_string() }
                            else if result.source.ends_with(".txt") { "txt".to_string() }
                            else if result.source.ends_with(".md") { "markdown".to_string() }
                            else { "document".to_string() }
                        });
                    
                    let file_size = result.metadata.get("size")
                        .and_then(|s| s.parse::<usize>().ok())
                        .unwrap_or_else(|| result.text.len());
                    
                    let sections = if let Some(heading) = &result.heading {
                        vec![heading.clone()]
                    } else {
                        vec!["Content".to_string()]
                    };
                    
                    let doc_info = serde_json::json!({
                        "id": result.doc_id.to_string(),
                        "title": result.title.clone(),
                        "source": result.source.clone(),
                        "type": doc_type,
                        "size": file_size,
                        "sections": sections,
                        "path": result.source.clone(),
                        "last_modified": result.metadata.get("modified_date")
                            .cloned()
                            .unwrap_or_else(|| chrono::Utc::now().to_rfc3339()),
                        "author": result.metadata.get("author").cloned(),
                        "department": result.metadata.get("department").cloned(),
                        "tags": result.metadata.get("tags")
                            .and_then(|t| serde_json::from_str::<Vec<String>>(t).ok())
                            .unwrap_or_default(),
                        "score": result.score,
                        "preview": if result.text.len() > 200 {
                            format!("{}...", &result.text[..200])
                        } else {
                            result.text.clone()
                        }
                    });
                    
                    source_docs.push(doc_info);
                } else {
                    // Fallback: Document not found in search, use ID info
                    source_docs.push(serde_json::json!({
                        "id": id_str,
                        "title": format!("Document {}", id_str.chars().take(8).collect::<String>()),
                        "type": "document",
                        "size": 0,
                        "sections": ["Unknown"],
                        "error": "Document metadata not found"
                    }));
                }
            } else {
                // Invalid UUID
                source_docs.push(serde_json::json!({
                    "id": id_str,
                    "title": "Invalid Document ID",
                    "type": "unknown",
                    "size": 0,
                    "sections": [],
                    "error": "Invalid document identifier"
                }));
            }
        }

    Ok(source_docs)
}

/// Generate HTML report with proper styling
async fn generate_html_report(
    request: GenerateDocumentRequest,
    rag_state: State<'_, RagState>,
    llm_state: State<'_, LLMState>,
) -> Result<String, String> {
    let default_query = String::from("General Report");
    let query = request.query.as_ref().unwrap_or(&default_query);

    // Perform RAG search
    let search_results = {
        let rag_guard = rag_state.rag.read().await;
        let rag = &*rag_guard;
        match rag.search(query, 5).await {
            Ok(results) => results,
            Err(e) => {
                tracing::warn!("RAG search failed: {}", e);
                vec![]
            }
        }
    };
    
    // Extract context
    let context: Vec<String> = search_results.iter()
        .map(|r| r.text.clone())
        .collect();

    // Check if we have content to work with
    if context.is_empty() {
        return Err(format!(
            "Cannot generate HTML report: No relevant information found in the knowledge base for '{}'. \
            Please add documents to your knowledge base first.",
            query
        ));
    }

    // Generate content using LLM with grounding rules
    let llm_content = {
        let manager_lock = llm_state.manager.read().await;
        if let Some(manager) = manager_lock.as_ref() {
            let doc_prompt = format!(
                "Create a professional report (plain text with clean structure, NOT raw HTML).\n\n\
                Topic: {query}\n\n\
                GROUNDING RULES (non-negotiable):\n\
                - You MUST use ONLY the Source Materials below. You have NO other knowledge.\n\
                - For EVERY claim, cite the source with [Source N] inline where N is the source number.\n\
                - If a fact is not explicitly stated in the Source Materials, DO NOT include it.\n\
                - NEVER infer, assume, or extrapolate beyond what the sources explicitly state.\n\
                - If the sources contain insufficient information for a section, write: \"Insufficient data in indexed documents.\"\n\n\
                FORMATTING RULES:\n\
                - NO emojis anywhere.\n\
                - Use proper paragraph spacing.\n\
                - Use bold for key terms and emphasis, not for entire sentences.\n\
                - Keep language concise, professional, and formal.\n\n\
                Document structure:\n\
                Executive Summary — 2-3 paragraph overview of key findings.\n\
                Background — Context and scope as stated in the sources.\n\
                Detailed Findings — Evidence with [Source N] citations.\n\
                Analysis — Interpretation strictly based on source evidence.\n\
                Recommendations — Actionable next steps supported by findings.\n\
                References — Numbered list of all sources used.\n\n\
                Source Materials:\n{context}",
                query = query,
                context = context.iter().enumerate()
                    .map(|(i, c)| format!("[Source {}]\n{}", i + 1, c))
                    .collect::<Vec<_>>()
                    .join("\n\n"),
            );

            match manager.generate_with_rag(&doc_prompt, context.clone()).await {
                Ok(generated) => generated,
                Err(e) => {
                    tracing::warn!("LLM generation failed: {}", e);
                    format!("<h2>Report on: {}</h2>\n<h3>Information Found:</h3>\n{}",
                        query,
                        context.iter()
                            .map(|c| format!("<p>{}</p>", c))
                            .collect::<Vec<_>>()
                            .join("\n")
                    )
                }
            }
        } else {
            format!("<h2>Report on: {}</h2>\n<h3>Information Found:</h3>\n{}",
                query,
                context.iter()
                    .map(|c| format!("<p>{}</p>", c))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        }
    };

    // Create sources HTML
    let sources_html = if !search_results.is_empty() {
        let source_items: Vec<String> = search_results.iter()
            .enumerate()
            .map(|(i, r)| {
                let title = r.metadata.get("title")
                    .cloned()
                    .unwrap_or_else(|| format!("Document {}", i + 1));
                let snippet = if r.text.len() > 200 {
                    format!("{}...", &r.text[..200])
                } else {
                    r.text.clone()
                };
                format!(
                    r#"
                    <div class="source-item">
                        <h4>Source {}: {}</h4>
                        <div class="relevance">Relevance Score: {:.2}</div>
                        <blockquote>{}</blockquote>
                    </div>
                    "#,
                    i + 1, title, r.score, snippet
                )
            })
            .collect();
        
        format!(
            r#"
            <section class="sources">
                <h2>Sources and References</h2>
                {}
            </section>
            "#,
            source_items.join("\n")
        )
    } else {
        String::from(r#"<section class="sources"><h2>Sources</h2><p><em>No sources were found for this query.</em></p></section>"#)
    };
    
    // Generate complete HTML document with embedded styles
    Ok(format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>{} Report</title>
    <style>
        body {{
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', 'Roboto', 'Helvetica Neue', Arial, sans-serif;
            line-height: 1.6;
            color: #333;
            background: white;
            margin: 0;
            padding: 40px;
            max-width: 900px;
            margin: 0 auto;
        }}
        
        .header {{
            border-bottom: 3px solid #2563eb;
            padding-bottom: 20px;
            margin-bottom: 30px;
        }}
        
        h1 {{
            color: #1e293b;
            font-size: 2.5em;
            margin-bottom: 10px;
        }}
        
        .meta {{
            color: #64748b;
            font-size: 14px;
        }}
        
        h2 {{
            color: #334155;
            font-size: 1.8em;
            margin-top: 30px;
            margin-bottom: 15px;
            border-bottom: 2px solid #e2e8f0;
            padding-bottom: 10px;
        }}
        
        h3 {{
            color: #475569;
            font-size: 1.4em;
            margin-top: 25px;
            margin-bottom: 10px;
        }}
        
        h4 {{
            color: #64748b;
            font-size: 1.2em;
            margin-top: 20px;
            margin-bottom: 8px;
        }}
        
        p {{
            margin-bottom: 15px;
            color: #334155;
        }}
        
        blockquote {{
            border-left: 4px solid #3b82f6;
            padding-left: 20px;
            margin: 20px 0;
            color: #475569;
            background: #f8fafc;
            padding: 15px 20px;
            border-radius: 4px;
        }}
        
        .executive-summary {{
            background: #eff6ff;
            border: 1px solid #dbeafe;
            border-radius: 8px;
            padding: 20px;
            margin: 20px 0;
        }}
        
        .content {{
            background: white;
            padding: 25px;
            margin: 20px 0;
            border-radius: 8px;
            box-shadow: 0 1px 3px rgba(0,0,0,0.1);
        }}
        
        .sources {{
            background: #fafafa;
            padding: 20px;
            margin-top: 40px;
            border-radius: 8px;
        }}
        
        .source-item {{
            background: white;
            padding: 15px;
            margin-bottom: 15px;
            border-radius: 6px;
            border: 1px solid #e5e7eb;
        }}
        
        .relevance {{
            color: #059669;
            font-size: 0.9em;
            font-weight: 600;
            margin: 5px 0;
        }}
        
        ul, ol {{
            margin-bottom: 15px;
            padding-left: 30px;
        }}
        
        li {{
            margin-bottom: 8px;
            color: #475569;
        }}
        
        .footer {{
            margin-top: 50px;
            padding-top: 20px;
            border-top: 1px solid #e5e7eb;
            text-align: center;
            color: #94a3b8;
            font-size: 12px;
        }}
        
        @media print {{
            body {{
                padding: 20px;
            }}
            .source-item {{
                page-break-inside: avoid;
            }}
        }}
    </style>
</head>
<body>
    <div class="header">
        <h1>{}</h1>
        <div class="meta">
            <strong>Date:</strong> {} | <strong>Query:</strong> {}
        </div>
    </div>

    <div class="content">
        <div>{}</div>
    </div>
    
    {}
    
    <div class="footer">
        <p>Generated by Kalki RAG System • Confidential Document</p>
    </div>
</body>
</html>"#,
        query.chars().take(50).collect::<String>(),
        query.chars().take(50).collect::<String>(),
        chrono::Utc::now().format("%B %d, %Y at %I:%M %p UTC"),
        query,
        llm_content.replace("\n", "<br>"),
        sources_html
    ))
}

/// Helper function to get report content
async fn get_report_content(
    query: &str,
    rag_state: State<'_, RagState>,
    llm_state: State<'_, LLMState>,
) -> Result<(Vec<SimpleSearchResult>, String), String> {
    // Perform RAG search
    let search_results = {
        let rag_guard = rag_state.rag.read().await;
        let rag = &*rag_guard;
        match rag.search(query, 5).await {
            Ok(results) => results,
            Err(e) => {
                tracing::warn!("RAG search failed: {}", e);
                vec![]
            }
        }
    };
    
    // Extract context
    let context: Vec<String> = search_results.iter()
        .map(|r| r.text.clone())
        .collect();
    
    // Require sources — refuse to generate from nothing
    if context.is_empty() {
        return Err(format!(
            "Cannot generate report: No relevant information found in the knowledge base for '{}'. \
            Please add documents to your knowledge base first.",
            query
        ));
    }

    // Generate content using LLM with grounding rules
    let llm_content = {
        let manager_lock = llm_state.manager.read().await;
        if let Some(manager) = manager_lock.as_ref() {
            let doc_prompt = format!(
                "Create a professional report.\n\n\
                Topic: {query}\n\n\
                GROUNDING RULES (non-negotiable):\n\
                - You MUST use ONLY the Source Materials below. You have NO other knowledge.\n\
                - For EVERY claim, cite the source with [Source N] inline where N is the source number.\n\
                - If a fact is not explicitly stated in the Source Materials, DO NOT include it.\n\
                - NEVER infer, assume, or extrapolate beyond what the sources explicitly state.\n\
                - If the sources contain insufficient information for a section, write: \"Insufficient data in indexed documents.\"\n\n\
                FORMATTING RULES (produce a pristine, publication-ready document):\n\
                - Use clean Markdown: # for title, ## for major sections, ### for subsections.\n\
                - NO emojis anywhere in the document.\n\
                - Use proper paragraph spacing — one blank line between paragraphs.\n\
                - Use **bold** for key terms and emphasis, not for entire sentences.\n\
                - Use bullet points or numbered lists for enumerations — not run-on paragraphs.\n\
                - Tables: use Markdown pipe tables for any structured/comparative data.\n\
                - Keep language concise, professional, and formal.\n\
                - End with a ## References section listing every source used (numbered to match inline citations).\n\n\
                Document structure:\n\
                ## Executive Summary\n\
                2-3 paragraph overview of key findings.\n\n\
                ## Background\n\
                Context and scope as stated in the sources.\n\n\
                ## Detailed Findings\n\
                Evidence and data with [Source N] citations. Use tables where data is comparative.\n\n\
                ## Analysis\n\
                Interpretation strictly based on source evidence.\n\n\
                ## Recommendations\n\
                Actionable next steps supported by findings.\n\n\
                ## References\n\
                Numbered list of all sources used.\n\n\
                Source Materials:\n{context}",
                query = query,
                context = context.iter().enumerate()
                    .map(|(i, c)| format!("[Source {}]\n{}", i + 1, c))
                    .collect::<Vec<_>>()
                    .join("\n\n"),
            );

            match manager.generate_with_rag(&doc_prompt, context.clone()).await {
                Ok(generated) => generated,
                Err(e) => {
                    format!("Unable to generate AI content: {}. Using context-based summary.", e)
                }
            }
        } else {
            context.join("\n\n")
        }
    };

    Ok((search_results, llm_content))
}

/// Helper function to wrap text for PDF
fn wrap_text(text: &str, max_chars: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        if paragraph.len() <= max_chars {
            lines.push(paragraph.to_string());
        } else {
            let words: Vec<&str> = paragraph.split_whitespace().collect();
            let mut current_line = String::new();
            
            for word in words {
                if current_line.len() + word.len() + 1 > max_chars {
                    if !current_line.is_empty() {
                        lines.push(current_line.clone());
                        current_line.clear();
                    }
                }
                if !current_line.is_empty() {
                    current_line.push(' ');
                }
                current_line.push_str(word);
            }
            
            if !current_line.is_empty() {
                lines.push(current_line);
            }
        }
    }
    lines
}

/// Generate actual PDF binary data
async fn generate_pdf_report(
    request: GenerateDocumentRequest,
    rag_state: State<'_, RagState>,
    llm_state: State<'_, LLMState>,
) -> Result<Vec<u8>, String> {
    let default_query = String::from("General Report");
    let query = request.query.as_ref().unwrap_or(&default_query);
    
    // Get the content first
    let (search_results, llm_content) = get_report_content(
        query,
        rag_state,
        llm_state
    ).await?;
    
    // Create PDF document
    let (doc, page1, layer1) = PdfDocument::new(
        &format!("{} Report", query),
        Mm(210.0), // A4 width
        Mm(297.0), // A4 height
        "Layer 1"
    );
    
    let current_layer = doc.get_page(page1).get_layer(layer1);
    
    // Load fonts
    let font = doc.add_builtin_font(BuiltinFont::Helvetica).map_err(|e| e.to_string())?;
    let font_bold = doc.add_builtin_font(BuiltinFont::HelveticaBold).map_err(|e| e.to_string())?;
    
    // Start position
    let mut y_position = Mm(280.0);
    
    // Title
    current_layer.use_text(
        &format!("{} Report", query),
        24.0,
        Mm(20.0),
        y_position,
        &font_bold
    );
    y_position -= Mm(10.0);
    
    // Date
    current_layer.use_text(
        &format!("Generated: {}", chrono::Utc::now().format("%B %d, %Y")),
        10.0,
        Mm(20.0),
        y_position,
        &font
    );
    y_position -= Mm(15.0);
    
    // Executive Summary heading
    current_layer.use_text(
        "Executive Summary",
        16.0,
        Mm(20.0),
        y_position,
        &font_bold
    );
    y_position -= Mm(8.0);
    
    // Content (wrapped)
    let content_lines = wrap_text(&llm_content, 80);
    for line in content_lines.iter().take(30) { // Limit to prevent overflow
        current_layer.use_text(
            line,
            11.0,
            Mm(20.0),
            y_position,
            &font
        );
        y_position -= Mm(5.0);
        
        if y_position < Mm(20.0) {
            break; // Need new page (simplified for now)
        }
    }
    
    // Sources section
    if !search_results.is_empty() && y_position > Mm(50.0) {
        y_position -= Mm(10.0);
        current_layer.use_text(
            "Sources",
            14.0,
            Mm(20.0),
            y_position,
            &font_bold
        );
        y_position -= Mm(8.0);
        
        for (i, result) in search_results.iter().take(3).enumerate() {
            let title = result.metadata.get("title")
                .cloned()
                .unwrap_or_else(|| format!("Source {}", i + 1));
            
            current_layer.use_text(
                &format!("• {} (Score: {:.2})", title, result.score),
                10.0,
                Mm(25.0),
                y_position,
                &font
            );
            y_position -= Mm(5.0);
        }
    }
    
    // Save to bytes
    let mut pdf_bytes = Vec::new();
    doc.save(&mut BufWriter::new(&mut pdf_bytes))
        .map_err(|e| format!("Failed to save PDF: {}", e))?;
    
    Ok(pdf_bytes)
}

/// Generate DOCX content
async fn generate_docx_content(
    request: GenerateDocumentRequest,
    rag_state: State<'_, RagState>,
    llm_state: State<'_, LLMState>,
) -> Result<Vec<u8>, String> {
    let default_query = String::from("General Report");
    let query = request.query.as_ref().unwrap_or(&default_query);
    
    // Get the content
    let (search_results, llm_content) = get_report_content(
        query,
        rag_state,
        llm_state
    ).await?;
    
    // Create DOCX document
    let mut docx = Docx::new();
    
    // Add title
    docx = docx.add_paragraph(
        Paragraph::new()
            .add_run(Run::new().add_text(&format!("{} Report", query)).size(32).bold())
    );
    
    // Add metadata
    docx = docx.add_paragraph(
        Paragraph::new()
            .add_run(Run::new().add_text(&format!(
                "Generated: {}",
                chrono::Utc::now().format("%B %d, %Y at %I:%M %p UTC")
            )).size(20).italic())
    );
    
    // Add executive summary
    docx = docx.add_paragraph(
        Paragraph::new()
            .add_run(Run::new().add_text("Executive Summary").size(28).bold())
    );
    
    docx = docx.add_paragraph(
        Paragraph::new()
            .add_run(Run::new().add_text(&llm_content).size(22))
    );
    
    // Add sources
    if !search_results.is_empty() {
        docx = docx.add_paragraph(
            Paragraph::new()
                .add_run(Run::new().add_text("Sources").size(28).bold())
        );
        
        for (i, result) in search_results.iter().enumerate() {
            let title = result.metadata.get("title")
                .cloned()
                .unwrap_or_else(|| format!("Source {}", i + 1));
            
            docx = docx.add_paragraph(
                Paragraph::new()
                    .add_run(Run::new().add_text(&format!(
                        "{}. {} (Score: {:.2})",
                        i + 1,
                        title,
                        result.score
                    )).size(20))
            );
            
            let snippet = if result.text.len() > 200 {
                format!("{}...", &result.text[..200])
            } else {
                result.text.clone()
            };
            
            docx = docx.add_paragraph(
                Paragraph::new()
                    .add_run(Run::new().add_text(&snippet).size(18).italic())
                    .indent(Some(400), None, None, None)
            );
        }
    }
    
    // Convert to bytes
    let mut docx_bytes = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut docx_bytes);
    docx.build().pack(&mut cursor)
        .map_err(|e| format!("Failed to generate DOCX: {}", e))?;
    
    Ok(docx_bytes)
}

/// Generate Excel/CSV spreadsheet content
async fn generate_spreadsheet_content(
    request: GenerateDocumentRequest,
    rag_state: State<'_, RagState>,
    llm_state: State<'_, LLMState>,
) -> Result<Vec<u8>, String> {
    let default_query = String::from("General Report");
    let query = request.query.as_ref().unwrap_or(&default_query);
    
    // Get the content
    let (search_results, llm_content) = get_report_content(
        query,
        rag_state,
        llm_state
    ).await?;
    
    // Create Excel workbook
    let mut workbook = Workbook::new();
    let worksheet = workbook.add_worksheet();
    
    // Add headers
    worksheet.write_string(0, 0, "Report Information")
        .map_err(|e| e.to_string())?;
    worksheet.write_string(1, 0, "Query")
        .map_err(|e| e.to_string())?;
    worksheet.write_string(1, 1, query)
        .map_err(|e| e.to_string())?;
    worksheet.write_string(2, 0, "Generated")
        .map_err(|e| e.to_string())?;
    worksheet.write_string(2, 1, &chrono::Utc::now().to_string())
        .map_err(|e| e.to_string())?;
    
    // Add summary
    worksheet.write_string(4, 0, "Executive Summary")
        .map_err(|e| e.to_string())?;
    worksheet.write_string(5, 0, &llm_content)
        .map_err(|e| e.to_string())?;
    
    // Add sources table
    worksheet.write_string(7, 0, "Sources")
        .map_err(|e| e.to_string())?;
    worksheet.write_string(8, 0, "Title")
        .map_err(|e| e.to_string())?;
    worksheet.write_string(8, 1, "Score")
        .map_err(|e| e.to_string())?;
    worksheet.write_string(8, 2, "Content Preview")
        .map_err(|e| e.to_string())?;
    
    for (i, result) in search_results.iter().enumerate() {
        let row = 9 + i as u32;
        let title = result.metadata.get("title")
            .cloned()
            .unwrap_or_else(|| result.id.to_string());
        
        worksheet.write_string(row, 0, &title)
            .map_err(|e| e.to_string())?;
        worksheet.write_number(row, 1, result.score as f64)
            .map_err(|e| e.to_string())?;
        
        let snippet = if result.text.len() > 100 {
            format!("{}...", &result.text[..100])
        } else {
            result.text.clone()
        };
        worksheet.write_string(row, 2, &snippet)
            .map_err(|e| e.to_string())?;
    }
    
    // Auto-fit columns
    worksheet.set_column_width(0, 20).map_err(|e| e.to_string())?;
    worksheet.set_column_width(1, 10).map_err(|e| e.to_string())?;
    worksheet.set_column_width(2, 60).map_err(|e| e.to_string())?;
    
    // Save to bytes
    let xlsx_bytes = workbook.save_to_buffer()
        .map_err(|e| format!("Failed to generate Excel: {}", e))?;
    
    Ok(xlsx_bytes)
}

/// Generate plain text report
async fn generate_text_report(
    request: GenerateDocumentRequest,
    rag_state: State<'_, RagState>,
    llm_state: State<'_, LLMState>,
) -> Result<String, String> {
    let default_query = String::from("General Report");
    let query = request.query.as_ref().unwrap_or(&default_query);
    
    // Get the content
    let (search_results, llm_content) = get_report_content(
        query,
        rag_state,
        llm_state
    ).await?;
    
    // Format as plain text
    let mut text = String::new();
    
    text.push_str(&"=".repeat(80));
    text.push_str(&format!("\n{} REPORT\n", query.to_uppercase()));
    text.push_str(&"=".repeat(80));
    text.push_str(&format!("\n\nGenerated: {}\n", chrono::Utc::now().format("%B %d, %Y at %I:%M %p UTC")));
    text.push_str(&format!("Query: {}\n\n", query));
    
    text.push_str(&"-".repeat(80));
    text.push_str("\nEXECUTIVE SUMMARY\n");
    text.push_str(&"-".repeat(80));
    text.push_str(&format!("\n\n{}\n\n", llm_content));
    
    if !search_results.is_empty() {
        text.push_str(&"-".repeat(80));
        text.push_str("\nSOURCES\n");
        text.push_str(&"-".repeat(80));
        text.push('\n');
        
        for (i, result) in search_results.iter().enumerate() {
            let title = result.metadata.get("title")
                .cloned()
                .unwrap_or_else(|| format!("Source {}", i + 1));
            
            text.push_str(&format!("\n[{}] {} (Score: {:.2})\n", i + 1, title, result.score));
            
            let snippet = if result.text.len() > 300 {
                format!("{}...", &result.text[..300])
            } else {
                result.text.clone()
            };
            text.push_str(&format!("    {}\n", snippet.replace('\n', "\n    ")));
        }
    }
    
    text.push_str(&format!("\n{}\n", "=".repeat(80)));
    text.push_str("END OF REPORT\n");
    
    Ok(text)
}

/// Generate JSON report
async fn generate_json_report(
    request: GenerateDocumentRequest,
    rag_state: State<'_, RagState>,
    llm_state: State<'_, LLMState>,
) -> Result<String, String> {
    let default_query = String::from("General Report");
    let query = request.query.as_ref().unwrap_or(&default_query);
    
    // Get the content
    let (search_results, llm_content) = get_report_content(
        query,
        rag_state,
        llm_state
    ).await?;
    
    // Create JSON structure
    let report = serde_json::json!({
        "report": {
            "title": format!("{} Report", query),
            "generated": chrono::Utc::now().to_rfc3339(),
            "query": query,
            "executive_summary": llm_content,
            "sources": search_results.iter().enumerate().map(|(i, r)| {
                serde_json::json!({
                    "index": i + 1,
                    "id": r.id,
                    "title": r.metadata.get("title").cloned().unwrap_or_else(|| r.id.to_string()),
                    "score": r.score,
                    "text_preview": if r.text.len() > 200 {
                        format!("{}...", &r.text[..200])
                    } else {
                        r.text.clone()
                    },
                    "metadata": r.metadata
                })
            }).collect::<Vec<_>>(),
            "statistics": {
                "total_sources": search_results.len(),
                "average_score": if search_results.is_empty() {
                    0.0
                } else {
                    search_results.iter().map(|r| r.score).sum::<f32>() / search_results.len() as f32
                }
            }
        }
    });
    
    serde_json::to_string_pretty(&report)
        .map_err(|e| format!("Failed to generate JSON: {}", e))
}

/// Generate document with streaming preview
/// Emits real-time tokens for live preview
#[tauri::command]
pub async fn generate_document_stream(
    app: tauri::AppHandle,
    prompt: String,
    format: String,
    rag_state: State<'_, RagState>,
    llm_state: State<'_, LLMState>,
) -> Result<String, String> {
    // Import not needed - we build events directly with serde_json

    let session_id = uuid::Uuid::new_v4().to_string();
    tracing::info!("📡 Starting streaming generation - Session: {}", session_id);

    // Step 1: Emit initial stage
    app.emit(&format!("generation_chunk_{}", session_id),
        serde_json::json!({
            "type": "Stage",
            "stage": "search",
            "message": "Searching knowledge base...",
            "progress": 10
        })
    ).map_err(|e| e.to_string())?;

    // Step 2: Perform RAG search
    let search_results = {
        let rag_guard = rag_state.rag.read().await;
        let rag = &*rag_guard;
        rag.search(&prompt, 10).await.map_err(|e| format!("Search failed: {}", e))?
    };

    // Step 3: Emit search complete
    let sources: Vec<_> = search_results.iter()
        .take(5)
        .map(|r| serde_json::json!({
            "title": r.metadata.get("title")
                .or_else(|| r.metadata.get("file_path"))
                .cloned()
                .unwrap_or_else(|| "Unknown".to_string()),
            "score": r.score,
        }))
        .collect();

    app.emit(&format!("generation_chunk_{}", session_id),
        serde_json::json!({
            "type": "SearchComplete",
            "num_sources": search_results.len(),
            "sources": sources
        })
    ).map_err(|e| e.to_string())?;

    // Step 4: Extract context
    let context: Vec<String> = search_results.iter()
        .map(|r| {
            let file_info = r.metadata.get("file_path")
                .map(|p| format!("[Source: {}]", p))
                .unwrap_or_default();
            format!("{}\n{}", file_info, r.text)
        })
        .collect();

    if context.is_empty() {
        app.emit(&format!("generation_chunk_{}", session_id),
            serde_json::json!({
                "type": "Error",
                "message": "No relevant information found in knowledge base. Please add documents first."
            })
        ).map_err(|e| e.to_string())?;
        return Err("No context found".to_string());
    }

    // Clone for async task
    let app_handle = app.clone();
    let session = session_id.clone();
    let prompt_clone = prompt.clone();
    let format_clone = format.clone();
    let context_for_llm = context.clone();
    let llm_manager_arc = llm_state.manager.clone();

    // Spawn LLM generation task
    tokio::spawn(async move {
        // Emit generation stage
        let _ = app_handle.emit(&format!("generation_chunk_{}", session),
            serde_json::json!({
                "type": "Stage",
                "stage": "generate",
                "message": "AI is writing document...",
                "progress": 50
            })
        );

        // Call LLM with context and grounding rules
        let llm_manager = llm_manager_arc.read().await;
        if let Some(manager) = llm_manager.as_ref() {
            let doc_prompt = format!(
                "Create a professional document.\n\n\
                Topic: {prompt}\n\n\
                GROUNDING RULES (non-negotiable):\n\
                - You MUST use ONLY the Source Materials below. You have NO other knowledge.\n\
                - For EVERY claim, cite the source with [Source N] inline where N is the source number.\n\
                - If a fact is not explicitly stated in the Source Materials, DO NOT include it.\n\
                - NEVER infer, assume, or extrapolate beyond what the sources explicitly state.\n\
                - An incomplete but 100% accurate document is better than a comprehensive but partially wrong one.\n\
                - If the sources contain insufficient information for a section, write: \"Insufficient data in indexed documents.\"\n\n\
                FORMATTING RULES (produce a pristine, publication-ready document):\n\
                - Use clean Markdown: # for title, ## for major sections, ### for subsections.\n\
                - NO emojis anywhere in the document.\n\
                - Use proper paragraph spacing — one blank line between paragraphs.\n\
                - Use **bold** for key terms and emphasis, not for entire sentences.\n\
                - Use bullet points or numbered lists for enumerations — not run-on paragraphs.\n\
                - Tables: use Markdown pipe tables for any structured/comparative data.\n\
                - Keep language concise, professional, and formal.\n\
                - End with a ## References section listing every source used (numbered to match inline citations).\n\n\
                Document structure:\n\
                ## Introduction\n\
                Brief overview of what the sources cover and scope of this document.\n\n\
                ## [Organize remaining content into logical sections based on the source material]\n\
                Use ## for major sections, ### for subsections. Include [Source N] citations.\n\n\
                ## Conclusion\n\
                Key takeaways from the sources.\n\n\
                ## References\n\
                Numbered list of all sources used.\n\n\
                Source Materials:\n{context}",
                prompt = prompt_clone,
                context = context_for_llm.iter().enumerate()
                    .map(|(i, c)| format!("[Source {}]\n{}", i + 1, c))
                    .collect::<Vec<_>>()
                    .join("\n\n"),
            );

            // Generate with RAG
            match manager.generate_with_rag(&doc_prompt, context_for_llm.clone()).await {
                Ok(content) => {
                    // Send content as batch
                    let _ = app_handle.emit(&format!("generation_chunk_{}", session),
                        serde_json::json!({
                            "type": "ContentBatch",
                            "content": content.clone()
                        })
                    );

                    // Complete
                    let _ = app_handle.emit(&format!("generation_chunk_{}", session),
                        serde_json::json!({
                            "type": "Complete",
                            "markdown": content,
                            "format": format_clone
                        })
                    );
                }
                Err(e) => {
                    tracing::warn!("LLM generation failed: {}", e);
                    let _ = app_handle.emit(&format!("generation_chunk_{}", session),
                        serde_json::json!({
                            "type": "Error",
                            "message": format!("LLM generation failed: {}", e)
                        })
                    );
                }
            }
        } else {
            // No LLM configured - create fallback document
            let fallback_content = format!(
                "# {} Report\n\n## Query\n{}\n\n## Found Information\n\n{}",
                prompt_clone,
                prompt_clone,
                context_for_llm.join("\n\n---\n\n")
            );

            let _ = app_handle.emit(&format!("generation_chunk_{}", session),
                serde_json::json!({
                    "type": "ContentBatch",
                    "content": fallback_content.clone()
                })
            );

            let _ = app_handle.emit(&format!("generation_chunk_{}", session),
                serde_json::json!({
                    "type": "Complete",
                    "markdown": fallback_content,
                    "format": format_clone
                })
            );
        }
    });

    Ok(session_id)
}

/// Document info returned to the frontend for template extraction and generation workflows
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComparableDocumentInfo {
    pub id: String,
    pub title: String,
    pub file_path: Option<String>,
    pub chunk_count: usize,
    pub space_id: Option<String>,
}

/// Get documents available in the knowledge base, optionally filtered by space.
/// Used by SmartTemplates to let users pick source documents for template extraction.
#[tauri::command]
pub async fn get_comparable_documents(
    space_id: Option<String>,
    rag_state: State<'_, RagState>,
) -> Result<Vec<ComparableDocumentInfo>, String> {
    tracing::info!("get_comparable_documents called (space_id: {:?})", space_id);

    let rag_guard = rag_state.rag.read().await;
    let rag = &*rag_guard;

    // List all chunks (no vector search, just metadata scan)
    let all_chunks = rag
        .list_documents(None, 100_000)
        .await
        .map_err(|e| format!("Failed to list documents: {}", e))?;

    // Group chunks by doc_id to build unique document list with counts
    let mut doc_map: std::collections::HashMap<String, ComparableDocumentInfo> =
        std::collections::HashMap::new();

    for chunk in &all_chunks {
        // Apply space filter if provided
        if let Some(ref sid) = space_id {
            let chunk_space = chunk.metadata.get("space_id");
            if chunk_space.map(|s| s != sid).unwrap_or(true) {
                continue;
            }
        }

        let doc_id = chunk.metadata.get("doc_id")
            .cloned()
            .unwrap_or_else(|| chunk.id.to_string());

        let entry = doc_map.entry(doc_id.clone()).or_insert_with(|| {
            let title = chunk.metadata.get("title")
                .or_else(|| chunk.metadata.get("file_name"))
                .cloned()
                .unwrap_or_else(|| {
                    chunk.metadata.get("file_path")
                        .map(|p| {
                            p.split(&['/', '\\'][..])
                                .last()
                                .unwrap_or("Unknown")
                                .to_string()
                        })
                        .unwrap_or_else(|| chunk.citation.title.clone())
                });

            ComparableDocumentInfo {
                id: doc_id,
                title,
                file_path: chunk.metadata.get("file_path").cloned(),
                chunk_count: 0,
                space_id: chunk.metadata.get("space_id").cloned(),
            }
        });

        entry.chunk_count += 1;
    }

    let mut docs: Vec<ComparableDocumentInfo> = doc_map.into_values().collect();
    docs.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));

    tracing::info!("Returning {} documents", docs.len());
    Ok(docs)
}