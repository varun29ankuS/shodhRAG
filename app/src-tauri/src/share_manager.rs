use serde::{Deserialize, Serialize};
use std::process::Command;
use anyhow::Result;
use base64;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareContent {
    pub title: String,
    pub content: String,
    pub citations: Vec<Citation>,
    pub format: ShareFormat,
    pub recipient: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Citation {
    pub text: String,
    pub source: String,
    pub page: Option<String>,
    pub paragraph: Option<String>,
    pub section: Option<String>,
    pub url: Option<String>,
    pub law_citation: Option<LegalCitation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LegalCitation {
    pub case_name: String,
    pub year: String,
    pub volume: Option<String>,
    pub reporter: String,  // SCC, AIR, etc.
    pub page: String,
    pub bench: Option<String>,
    pub para: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ShareFormat {
    PlainText,
    PDF,
    Word,
    HTML,
    Markdown,
    WhatsAppFormatted,
}

pub struct ShareManager;

impl ShareManager {
    pub fn format_with_citations(content: &ShareContent) -> String {
        let mut formatted = format!("📋 {}\n\n", content.title);
        formatted.push_str(&content.content);
        formatted.push_str("\n\n");

        if !content.citations.is_empty() {
            formatted.push_str("📚 Citations:\n");
            for (i, citation) in content.citations.iter().enumerate() {
                formatted.push_str(&format!("{}. {}\n", i + 1, Self::format_citation(citation)));
            }
        }

        formatted
    }

    fn format_citation(citation: &Citation) -> String {
        if let Some(legal) = &citation.law_citation {
            // Legal citation format: Case Name (Year) Volume Reporter Page
            format!(
                "{} ({}) {} {} {}{}{}",
                legal.case_name,
                legal.year,
                legal.volume.as_deref().unwrap_or(""),
                legal.reporter,
                legal.page,
                legal.bench.as_ref().map(|b| format!(" [{}]", b)).unwrap_or_default(),
                legal.para.as_ref().map(|p| format!(", Para {}", p)).unwrap_or_default()
            )
        } else {
            // Academic/general citation
            format!(
                "{}{}{}{}",
                citation.source,
                citation.page.as_ref().map(|p| format!(", p. {}", p)).unwrap_or_default(),
                citation.section.as_ref().map(|s| format!(", Section {}", s)).unwrap_or_default(),
                citation.paragraph.as_ref().map(|p| format!(", Para {}", p)).unwrap_or_default()
            )
        }
    }

    pub async fn share_to_whatsapp(content: &ShareContent, phone: Option<String>) -> Result<()> {
        let formatted = Self::format_with_citations(content);

        // URL encode the message
        let encoded_msg = urlencoding::encode(&formatted);

        let url = if let Some(phone_number) = phone {
            // Direct to specific number (remove +91 or 91 prefix if present)
            let clean_number = phone_number
                .replace("+91", "")
                .replace("91", "")
                .replace(" ", "")
                .replace("-", "");
            format!("https://wa.me/91{}?text={}", clean_number, encoded_msg)
        } else {
            // Open WhatsApp Web with pre-filled message
            format!("https://wa.me/?text={}", encoded_msg)
        };

        // Open in default browser
        #[cfg(target_os = "windows")]
        {
            Command::new("cmd")
                .args(["/C", "start", &url])
                .spawn()?;
        }

        #[cfg(target_os = "macos")]
        {
            Command::new("open").arg(&url).spawn()?;
        }

        #[cfg(target_os = "linux")]
        {
            Command::new("xdg-open").arg(&url).spawn()?;
        }

        Ok(())
    }

    pub async fn share_via_email(
        content: &ShareContent,
        recipient: Option<String>,
        attachment: Option<Vec<u8>>,
    ) -> Result<()> {
        let formatted = Self::format_with_citations(content);

        // Create mailto URL with proper encoding
        let subject = urlencoding::encode(&content.title);
        let body = urlencoding::encode(&formatted);

        let mut mailto = format!("mailto:{}?subject={}&body={}",
            recipient.unwrap_or_default(),
            subject,
            body
        );

        // If we have an attachment, we need to save it temporarily
        if let Some(file_data) = attachment {
            let temp_dir = std::env::temp_dir();
            let file_path = temp_dir.join(format!("{}.pdf", content.title.replace(" ", "_")));
            std::fs::write(&file_path, file_data)?;

            // Note: mailto doesn't support attachments directly
            // We'll open the email client and user needs to attach manually
            tracing::info!("File saved for attachment: {:?}", file_path);
        }

        // Open default email client
        #[cfg(target_os = "windows")]
        {
            Command::new("cmd")
                .args(["/C", "start", &mailto.replace("&", "^&")])
                .spawn()?;
        }

        #[cfg(target_os = "macos")]
        {
            Command::new("open").arg(&mailto).spawn()?;
        }

        #[cfg(target_os = "linux")]
        {
            Command::new("xdg-open").arg(&mailto).spawn()?;
        }

        Ok(())
    }

    pub fn generate_citation_block(citations: &[Citation]) -> String {
        let mut block = String::from("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");
        block.push_str("📚 REFERENCES AND CITATIONS\n");
        block.push_str("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n\n");

        for (i, citation) in citations.iter().enumerate() {
            block.push_str(&format!("[{}] {}\n", i + 1, Self::format_citation(citation)));
            if let Some(url) = &citation.url {
                block.push_str(&format!("    URL: {}\n", url));
            }
            block.push_str("\n");
        }

        block
    }

    pub fn format_for_whatsapp(content: &str, citations: &[Citation]) -> String {
        let mut formatted = String::new();

        // WhatsApp formatting
        // Bold: *text*
        // Italic: _text_
        // Strikethrough: ~text~
        // Monospace: ```text```

        // Process content
        formatted.push_str(content);
        formatted.push_str("\n\n");

        if !citations.is_empty() {
            formatted.push_str("*Citations:*\n");
            for citation in citations {
                formatted.push_str(&format!("• {}\n", Self::format_citation(citation)));
            }
        }

        formatted
    }

    pub fn copy_to_clipboard(content: &str) -> Result<()> {
        #[cfg(target_os = "windows")]
        {
            use clipboard_win::{formats, set_clipboard};
            set_clipboard(formats::Unicode, content)?;
        }

        #[cfg(not(target_os = "windows"))]
        {
            use arboard::Clipboard;
            let mut clipboard = Clipboard::new()?;
            clipboard.set_text(content)?;
        }

        Ok(())
    }
}

// Tauri commands
#[tauri::command]
pub async fn share_to_whatsapp_cmd(
    content: ShareContent,
    phone: Option<String>,
) -> Result<(), String> {
    ShareManager::share_to_whatsapp(&content, phone)
        .await
        .map_err(|e| format!("Failed to share to WhatsApp: {}", e))
}

#[tauri::command]
pub async fn share_via_email_cmd(
    content: ShareContent,
    recipient: Option<String>,
    attachment_base64: Option<String>,
) -> Result<(), String> {
    let attachment = attachment_base64
        .map(|b64| base64::decode(b64))
        .transpose()
        .map_err(|e| format!("Failed to decode attachment: {}", e))?;

    ShareManager::share_via_email(&content, recipient, attachment)
        .await
        .map_err(|e| format!("Failed to share via email: {}", e))
}

#[tauri::command]
pub fn copy_with_citations(
    content: String,
    citations: Vec<Citation>,
) -> Result<(), String> {
    let formatted = format!(
        "{}\n\n{}",
        content,
        ShareManager::generate_citation_block(&citations)
    );

    ShareManager::copy_to_clipboard(&formatted)
        .map_err(|e| format!("Failed to copy to clipboard: {}", e))
}

#[tauri::command]
pub fn format_legal_citation(
    case_name: String,
    year: String,
    reporter: String,
    page: String,
    para: Option<String>,
) -> String {
    let citation = Citation {
        text: format!("{} case", case_name),
        source: case_name.clone(),
        page: Some(page.clone()),
        paragraph: para.clone(),
        section: None,
        url: None,
        law_citation: Some(LegalCitation {
            case_name,
            year,
            volume: None,
            reporter,
            page,
            bench: None,
            para,
        }),
    };

    ShareManager::format_citation(&citation)
}