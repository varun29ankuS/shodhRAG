//! Export forms as downloadable HTML or JSON files

use super::structured_output::{FieldType, FormField};
use anyhow::Result;

/// Export form as standalone HTML file with embedded CSS and JavaScript
pub fn export_form_as_html(
    title: &str,
    description: Option<&str>,
    fields: &[FormField],
) -> Result<String> {
    let desc_html = description
        .map(|d| format!("    <p class=\"description\">{}</p>\n", html_escape(d)))
        .unwrap_or_default();

    let mut html = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>{title}</title>
    <style>
        * {{
            margin: 0;
            padding: 0;
            box-sizing: border-box;
        }}

        body {{
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, 'Helvetica Neue', Arial, sans-serif;
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            min-height: 100vh;
            display: flex;
            align-items: center;
            justify-content: center;
            padding: 20px;
        }}

        .container {{
            background: white;
            border-radius: 12px;
            box-shadow: 0 20px 60px rgba(0, 0, 0, 0.3);
            max-width: 600px;
            width: 100%;
            padding: 40px;
        }}

        h1 {{
            color: #333;
            margin-bottom: 10px;
            font-size: 28px;
        }}

        .description {{
            color: #666;
            margin-bottom: 30px;
            line-height: 1.6;
        }}

        .form-group {{
            margin-bottom: 24px;
        }}

        label {{
            display: block;
            margin-bottom: 8px;
            font-weight: 600;
            color: #444;
            font-size: 14px;
        }}

        .required {{
            color: #e74c3c;
            margin-left: 4px;
        }}

        input[type="text"],
        input[type="email"],
        input[type="number"],
        input[type="date"],
        input[type="tel"],
        input[type="url"],
        select,
        textarea {{
            width: 100%;
            padding: 12px 16px;
            border: 2px solid #e0e0e0;
            border-radius: 8px;
            font-size: 15px;
            transition: all 0.3s ease;
            font-family: inherit;
        }}

        input:focus,
        select:focus,
        textarea:focus {{
            outline: none;
            border-color: #667eea;
            box-shadow: 0 0 0 3px rgba(102, 126, 234, 0.1);
        }}

        textarea {{
            min-height: 120px;
            resize: vertical;
        }}

        .checkbox-group,
        .radio-group {{
            display: flex;
            flex-direction: column;
            gap: 10px;
        }}

        .checkbox-group label,
        .radio-group label {{
            display: flex;
            align-items: center;
            font-weight: normal;
            cursor: pointer;
        }}

        input[type="checkbox"],
        input[type="radio"] {{
            margin-right: 10px;
            width: 18px;
            height: 18px;
            cursor: pointer;
        }}

        button {{
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            color: white;
            padding: 14px 32px;
            border: none;
            border-radius: 8px;
            font-size: 16px;
            font-weight: 600;
            cursor: pointer;
            width: 100%;
            transition: transform 0.2s ease, box-shadow 0.2s ease;
        }}

        button:hover {{
            transform: translateY(-2px);
            box-shadow: 0 8px 16px rgba(102, 126, 234, 0.3);
        }}

        button:active {{
            transform: translateY(0);
        }}

        .success-message {{
            display: none;
            background: #d4edda;
            color: #155724;
            padding: 16px;
            border-radius: 8px;
            margin-top: 20px;
            border: 1px solid #c3e6cb;
        }}

        .success-message.show {{
            display: block;
        }}
    </style>
</head>
<body>
    <div class="container">
        <h1>{title}</h1>
{desc_html}
        <form id="generatedForm">
"#
    );

    // Generate form fields
    for field in fields {
        html.push_str(&format!("            <div class=\"form-group\">\n"));
        html.push_str(&format!(
            "                <label for=\"{}\">{}{}</label>\n",
            field.id,
            html_escape(&field.label),
            if field.required {
                " <span class=\"required\">*</span>"
            } else {
                ""
            }
        ));

        match field.field_type {
            FieldType::Text
            | FieldType::Email
            | FieldType::Number
            | FieldType::Date
            | FieldType::Tel
            | FieldType::Url => {
                let input_type = match field.field_type {
                    FieldType::Text => "text",
                    FieldType::Email => "email",
                    FieldType::Number => "number",
                    FieldType::Date => "date",
                    FieldType::Tel => "tel",
                    FieldType::Url => "url",
                    _ => "text",
                };

                html.push_str(&format!(
                    "                <input type=\"{}\" id=\"{}\" name=\"{}\" {} {} />\n",
                    input_type,
                    field.id,
                    field.id,
                    if field.required { "required" } else { "" },
                    field
                        .placeholder
                        .as_ref()
                        .map(|p| format!("placeholder=\"{}\"", html_escape(p)))
                        .unwrap_or_default()
                ));
            }

            FieldType::Textarea => {
                html.push_str(&format!(
                    "                <textarea id=\"{}\" name=\"{}\" {}>{}</textarea>\n",
                    field.id,
                    field.id,
                    if field.required { "required" } else { "" },
                    field.placeholder.as_deref().unwrap_or("")
                ));
            }

            FieldType::Select => {
                html.push_str(&format!(
                    "                <select id=\"{}\" name=\"{}\" {}>\n",
                    field.id,
                    field.id,
                    if field.required { "required" } else { "" }
                ));
                html.push_str(
                    "                    <option value=\"\">-- Select an option --</option>\n",
                );

                if let Some(options) = &field.options {
                    for option in options {
                        html.push_str(&format!(
                            "                    <option value=\"{}\">{}</option>\n",
                            html_escape(option),
                            html_escape(option)
                        ));
                    }
                }

                html.push_str("                </select>\n");
            }

            FieldType::Checkbox => {
                html.push_str("                <div class=\"checkbox-group\">\n");
                if let Some(options) = &field.options {
                    for option in options {
                        let option_id = format!("{}_{}", field.id, sanitize_id(option));
                        html.push_str(&format!(
                            "                    <label>\n                        <input type=\"checkbox\" id=\"{}\" name=\"{}\" value=\"{}\" />\n                        {}\n                    </label>\n",
                            option_id,
                            field.id,
                            html_escape(option),
                            html_escape(option)
                        ));
                    }
                }
                html.push_str("                </div>\n");
            }

            FieldType::Radio => {
                html.push_str("                <div class=\"radio-group\">\n");
                if let Some(options) = &field.options {
                    for option in options {
                        let option_id = format!("{}_{}", field.id, sanitize_id(option));
                        html.push_str(&format!(
                            "                    <label>\n                        <input type=\"radio\" id=\"{}\" name=\"{}\" value=\"{}\" {} />\n                        {}\n                    </label>\n",
                            option_id,
                            field.id,
                            html_escape(option),
                            if field.required { "required" } else { "" },
                            html_escape(option)
                        ));
                    }
                }
                html.push_str("                </div>\n");
            }
        }

        html.push_str("            </div>\n");
    }

    // Add submit button and JavaScript
    html.push_str(
        r#"
            <button type="submit">Submit Form</button>
        </form>

        <div class="success-message" id="successMessage">
            ✓ Form submitted successfully! Check the browser console for data.
        </div>
    </div>

    <script>
        document.getElementById('generatedForm').addEventListener('submit', function(e) {
            e.preventDefault();

            // Collect form data
            const formData = new FormData(this);
            const data = {};

            // Handle checkboxes specially (multiple values)
            const checkboxFields = new Set();
            this.querySelectorAll('input[type="checkbox"]').forEach(cb => {
                if (!data[cb.name]) {
                    data[cb.name] = [];
                    checkboxFields.add(cb.name);
                }
            });

            // Collect all form values
            for (const [key, value] of formData.entries()) {
                if (checkboxFields.has(key)) {
                    data[key].push(value);
                } else {
                    data[key] = value;
                }
            }

            console.log('Form Data:', JSON.stringify(data, null, 2));

            // Show success message
            const successMsg = document.getElementById('successMessage');
            successMsg.classList.add('show');

            // Hide after 3 seconds
            setTimeout(() => {
                successMsg.classList.remove('show');
            }, 3000);

            // Optional: Send to server
            // fetch('/api/submit', {
            //     method: 'POST',
            //     headers: { 'Content-Type': 'application/json' },
            //     body: JSON.stringify(data)
            // });
        });
    </script>
</body>
</html>
"#,
    );

    Ok(html)
}

/// Export form as JSON Schema for integration with other systems
pub fn export_form_as_json_schema(
    title: &str,
    description: Option<&str>,
    fields: &[FormField],
) -> Result<String> {
    use serde_json::json;

    let properties: serde_json::Map<String, serde_json::Value> = fields
        .iter()
        .map(|f| {
            let mut prop = match f.field_type {
                FieldType::Text
                | FieldType::Email
                | FieldType::Tel
                | FieldType::Url
                | FieldType::Textarea => {
                    json!({ "type": "string" })
                }
                FieldType::Number => json!({ "type": "number" }),
                FieldType::Date => json!({ "type": "string", "format": "date" }),
                FieldType::Select | FieldType::Radio => {
                    if let Some(options) = &f.options {
                        json!({ "type": "string", "enum": options })
                    } else {
                        json!({ "type": "string" })
                    }
                }
                FieldType::Checkbox => {
                    json!({ "type": "array", "items": { "type": "string" } })
                }
            };

            if let Some(obj) = prop.as_object_mut() {
                obj.insert("title".to_string(), json!(f.label));
                if let Some(placeholder) = &f.placeholder {
                    obj.insert("description".to_string(), json!(placeholder));
                }
            }

            (f.id.clone(), prop)
        })
        .collect();

    let required: Vec<&str> = fields
        .iter()
        .filter(|f| f.required)
        .map(|f| f.id.as_str())
        .collect();

    let schema = json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "title": title,
        "description": description,
        "type": "object",
        "properties": properties,
        "required": required
    });

    Ok(serde_json::to_string_pretty(&schema)?)
}

/// Escape HTML special characters
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Sanitize string for use in HTML ID attributes
fn sanitize_id(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rag::structured_output::{FieldType, FormField};

    #[test]
    fn test_export_simple_form() {
        let fields = vec![
            FormField {
                id: "name".to_string(),
                field_type: FieldType::Text,
                label: "Full Name".to_string(),
                required: true,
                placeholder: Some("Enter your name".to_string()),
                options: None,
                default_value: None,
            },
            FormField {
                id: "email".to_string(),
                field_type: FieldType::Email,
                label: "Email Address".to_string(),
                required: true,
                placeholder: Some("you@example.com".to_string()),
                options: None,
                default_value: None,
            },
        ];

        let html = export_form_as_html("Test Form", Some("Test description"), &fields).unwrap();

        assert!(html.contains("Test Form"));
        assert!(html.contains("Test description"));
        assert!(html.contains("Full Name"));
        assert!(html.contains("Email Address"));
        assert!(html.contains("type=\"text\""));
        assert!(html.contains("type=\"email\""));
    }

    #[test]
    fn test_export_json_schema() {
        let fields = vec![FormField {
            id: "age".to_string(),
            field_type: FieldType::Number,
            label: "Age".to_string(),
            required: true,
            placeholder: None,
            options: None,
            default_value: None,
        }];

        let json = export_form_as_json_schema("Age Form", None, &fields).unwrap();

        assert!(json.contains("Age Form"));
        assert!(json.contains("\"type\": \"number\""));
        assert!(json.contains("\"required\""));
    }
}
