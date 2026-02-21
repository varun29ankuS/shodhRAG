//! Structured output parsing for LLM responses
//! Enables LLM to generate tables, charts, forms, and system actions that backend can parse and validate

use serde::{Deserialize, Serialize};
use anyhow::{Result, Context};
use crate::system::file_ops::{FileSystemAction, FileSystemResult};
use crate::system::command_executor::{CommandAction, CommandResult};

/// Structured output types that LLM can generate
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum StructuredOutput {
    /// Plain text content
    Text {
        content: String
    },

    /// Table with headers and rows
    Table {
        headers: Vec<String>,
        rows: Vec<Vec<String>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        caption: Option<String>,
    },

    /// Chart/graph specification
    Chart {
        chart_type: ChartType,
        title: String,
        data: ChartData,
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },

    /// Diagram using Mermaid syntax (flowchart, sequence, class, ER, state, gantt, git, journey)
    Diagram {
        diagram_type: DiagramType,
        title: String,
        mermaid: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },

    /// Form definition for user input
    Form {
        title: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        fields: Vec<FormField>,
    },

    /// System action (file operations, commands)
    #[serde(rename = "system_action")]
    SystemAction {
        action: SystemActionType,
    },
}

/// System action types (file ops or commands)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SystemActionType {
    FileSystem(FileSystemAction),
    Command(CommandAction),
}

/// Supported chart types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChartType {
    Bar,
    Line,
    Pie,
    Scatter,
    Area,
    Radar,
    Doughnut,
    Bubble,
}

/// Supported diagram types (Mermaid.js)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagramType {
    Flowchart,   // Process flows, architecture diagrams
    Sequence,    // API interactions, system communications
    Class,       // OOP structure, code architecture
    #[serde(rename = "er")]
    ER,          // Entity-Relationship, database schemas
    State,       // State machines, workflow states
    Gantt,       // Project timelines, schedules
    Git,         // Version control flows, branching
    Journey,     // User journeys, experience flows
}

/// Chart data structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChartData {
    pub labels: Vec<String>,
    pub datasets: Vec<Dataset>,
}

/// Dataset for charts
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dataset {
    pub label: String,
    pub data: Vec<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background_color: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub border_color: Option<Vec<String>>,
}

/// Form field definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormField {
    pub id: String,
    #[serde(rename = "type")]
    pub field_type: FieldType,
    pub label: String,
    pub required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_value: Option<String>,
}

/// Supported form field types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FieldType {
    Text,
    Number,
    Email,
    Date,
    Select,
    Checkbox,
    Radio,
    Textarea,
    Tel,
    Url,
}

/// System prompt instructions for LLM to generate structured outputs
pub const STRUCTURED_OUTPUT_INSTRUCTIONS: &str = r##"
# CRITICAL: STRUCTURED OUTPUT GENERATION

IMPORTANT: When presenting data, you MUST use structured code blocks, not plain text descriptions.

DO NOT say "Here is a table" - SHOW THE TABLE using ```table blocks.
DO NOT say "I'll create a chart" - OUTPUT THE CHART using ```chart blocks.

You can generate structured outputs for better data presentation using code blocks:

## TABLES
When data is best shown as a table, use markdown tables in a code block starting with three backticks and "table":

**CRITICAL SYNTAX**: Start with ```table (three backticks + "table"), then newline, then markdown table, then closing ```

CORRECT FORMAT:
```table
| Header 1 | Header 2 | Header 3 |
|----------|----------|----------|
| Value 1  | Value 2  | Value 3  |
| Value 4  | Value 5  | Value 6  |
```

## CHARTS
When data should be visualized, you MUST use JSON in a code block starting with three backticks and the word "chart":

**CRITICAL SYNTAX**: Start with ```chart (three backticks + "chart"), then newline, then JSON, then closing ```

CORRECT FORMAT:
```chart
{
  "type": "bar",
  "title": "Sales by Region",
  "data": {
    "labels": ["North", "South", "East", "West"],
    "datasets": [{
      "label": "Q1 2024",
      "data": [45000, 67000, 52000, 39000]
    }]
  }
}
```

WRONG FORMAT (DO NOT USE):
❌ chart { "type": "bar" ... } (missing backticks!)
❌ "Here's a chart: {..." (not in code block!)

Supported chart types: bar, line, pie, scatter, area, radar, doughnut, bubble

## DIAGRAMS (Mermaid.js)
Visualize processes, workflows, systems, and data models using Mermaid syntax. Choose the appropriate diagram type:

### 1. FLOWCHARTS (```flowchart) - Process flows, algorithms, workflows
SYNTAX RULES - FOLLOW EXACTLY:
- Direction: `graph LR` (left-right) or `graph TD` (top-down)
- Node shapes: `[Square]`, `(Round)`, `([Stadium])`, `[[Subroutine]]`, `{Diamond}`
- Arrows: Use `-->` (solid), `-.->` (dotted), `==>` (thick)
- Arrow labels: Use `-->|label|` syntax ONLY. DO NOT use `-- label -->` or parentheses
- Node IDs: Alphanumeric only (A, B, C, Node1, Step2). NO SPACES.
- **CRITICAL TEXT RULES**:
  - NO quotes (") inside node text - use single words or hyphens
  - NO parentheses, commas, or colons in text
  - Use simple, short labels (max 3-4 words)
  - Replace "User Already Exists" with "User-Exists" or "UserExists"

CORRECT EXAMPLE - RAG System:
```flowchart
graph TD
    A[User Query] -->|embed| B[Vector Embedding]
    B -->|search| C[Vector Database]
    C -->|retrieve| D[Top-K Results]
    D -->|rank| E[Reranking Engine]
    E -->|augment| F[Context Builder]
    F -->|generate| G[LLM Response]
    style A fill:#e74c3c,color:#fff
    style G fill:#2ecc71,color:#fff
```

WRONG EXAMPLES:
❌ `I[Display "User Already Exists"]` - quotes break parser!
✅ `I[Display User-Exists]` - correct, no quotes

### 2. SEQUENCE DIAGRAMS (```sequence) - API calls, system interactions, communication flows
```sequence
sequenceDiagram
    User->>API: Login Request
    API->>Database: Check Credentials
    Database-->>API: User Data
    API-->>User: JWT Token
```

### 3. CLASS DIAGRAMS (```class) - OOP structure, code architecture, relationships
```class
classDiagram
    Animal <|-- Duck
    Animal <|-- Fish
    Animal : +int age
    Animal: +isMammal()
    class Duck{
        +String beakColor
        +swim()
    }
```

### 4. ER DIAGRAMS (```erdiagram or ```er) - Database schemas, data models, entity relationships
```erdiagram
erDiagram
    CUSTOMER ||--o{ ORDER : places
    ORDER ||--|{ LINE-ITEM : contains
    CUSTOMER }|..|{ DELIVERY-ADDRESS : uses
```

### 5. STATE DIAGRAMS (```state) - State machines, workflow states, transitions
```state
stateDiagram-v2
    [*] --> Idle
    Idle --> Processing : Start
    Processing --> Success : Complete
    Processing --> Failed : Error
    Success --> [*]
```

### 6. GANTT CHARTS (```gantt) - Project timelines, schedules, milestones
```gantt
gantt
    title Project Timeline
    section Phase 1
    Design :a1, 2024-01-01, 30d
    Development :after a1, 45d
```

### 7. GIT GRAPHS (```gitgraph or ```git) - Version control, branching strategies
```gitgraph
gitGraph
    commit id: "Initial"
    branch develop
    commit id: "Feature 1"
    checkout main
    commit id: "Hotfix"
    merge develop tag: "v1.0"
```
IMPORTANT: Use lowercase keywords: commit, branch, checkout, merge. Tag commits with `tag: "v1.0"`

### 8. USER JOURNEYS (```journey) - User experience flows, interaction paths
```journey
journey
    title User Login Journey
    section Access Site
      Visit homepage: 5: User
      Click login: 4: User
    section Authentication
      Enter credentials: 3: User
      Verify identity: 2: System
```

CRITICAL SYNTAX RULES (ALL DIAGRAMS):
- Node/Entity labels: MAXIMUM 3 WORDS
- **ABSOLUTELY NO QUOTES (")** in any node text - this breaks the parser!
- NO commas, parentheses, "etc", or colons in text (except in gitGraph commit IDs)
- Use hyphens or camelCase instead: "Already Exists" → "Already-Exists" or "AlreadyExists"
- Keep diagrams compact (5-8 elements max)
- Add emojis for visual appeal: 🔍 📊 💾 🔄 ⚡ 🎯 ✨

SPECIFIC SYNTAX RULES:
- **Flowchart**: Use `graph LR` or `graph TD`, square brackets for nodes `[Text]`
  - CORRECT: `A[Text] -->|label| B[More Text]`
  - WRONG: `A --> B[Text] (Note)` - parentheses break parsing
  - WRONG: `A -- Label --> B` - old syntax not supported
- **Sequence**: Use arrows `->` `->>` `-->>`, Actor names without quotes
- **Class**: Use inheritance `<|--`, composition `*--`, relationships `-->`
- **ER**: Use cardinality `||--o{` `}o--||`, relationship labels in quotes
- **State**: Use `[*]` for start/end, arrows with `:` for transitions
- **Gantt**: Use `section`, dates in YYYY-MM-DD, `:after taskId` for dependencies
- **Git**: Use lowercase `commit` `branch` `checkout` `merge`, tags with `tag: "v1.0"`
- **Journey**: Use `section`, score from 1-5, Actor name after score

CHOOSE THE RIGHT DIAGRAM:
- Process/workflow? → ```flowchart
- API/system interaction? → ```sequence
- Code structure? → ```class
- Database schema? → ```erdiagram
- Workflow states? → ```state
- Project timeline? → ```gantt
- Git branching? → ```gitgraph
- User experience? → ```journey

## FORMS
When creating fillable forms, use JSON in a ```form block:
```form
{
  "title": "Employee Onboarding",
  "description": "Please fill in your details",
  "fields": [
    {
      "id": "name",
      "type": "text",
      "label": "Full Name",
      "required": true,
      "placeholder": "Enter your full name"
    },
    {
      "id": "department",
      "type": "select",
      "label": "Department",
      "required": true,
      "options": ["Engineering", "Sales", "Marketing", "HR"]
    }
  ]
}
```

Supported field types: text, number, email, date, select, checkbox, radio, textarea, tel, url

## SYSTEM ACTIONS (File & OS Operations)
You can interact with the operating system using JSON in ```action blocks:

### Create Folders
```action
{
  "type": "create_folders",
  "base_path": "C:/Projects/MyApp",
  "structure": {
    "src": ["components", "pages", "utils"],
    "public": ["assets"],
    "tests": []
  }
}
```

### Create File
```action
{
  "type": "create_file",
  "path": "C:/Projects/MyApp/README.md",
  "content": "# My Application\n\nCreated by Shodh AI.",
  "overwrite": false
}
```

### Run PowerShell (Windows) - Safe commands only
```action
{
  "type": "powershell",
  "command": "Get-Process | Where-Object CPU -gt 10 | Select-Object Name,CPU",
  "description": "List high CPU processes"
}
```

### Run Bash (Unix/Mac)
```action
{
  "type": "bash",
  "command": "ls -la /home/user/projects",
  "description": "List project directory"
}
```

### List Directory
```action
{
  "type": "list_directory",
  "path": "C:/Users/Downloads",
  "recursive": false
}
```

CRITICAL: When the user requests a system operation (get processes, list files, create folders, run commands):
1. IMMEDIATELY output the ```action block - DO NOT ask for permission in text
2. The UI will automatically show an approval dialog with the action details
3. The user will click "Approve & Execute" or "Deny" buttons
4. DO NOT wait for "yes" - just output the ```action block directly

BAD (Don't do this):
"This action will retrieve a list of all running processes. Would you like me to execute this command?"

GOOD (Do this):
```action
{
  "type": "powershell",
  "command": "Get-Process | Select-Object Name,CPU,WorkingSet | Sort-Object CPU -Descending",
  "description": "List all running processes with CPU and memory usage"
}
```

You can freely generate system actions. The user has final control via the approval dialog.

Examples:

User: "Compare Q1 vs Q2 sales across regions"
Assistant: Here's the sales comparison:

```table
| Region | Q1 Sales | Q2 Sales | Growth |
|--------|----------|----------|--------|
| North  | $45k     | $67k     | +49%   |
| South  | $67k     | $89k     | +33%   |
| East   | $52k     | $78k     | +50%   |
| West   | $39k     | $56k     | +44%   |
```

```chart
{
  "type": "bar",
  "title": "Q1 vs Q2 Sales Comparison",
  "data": {
    "labels": ["North", "South", "East", "West"],
    "datasets": [
      {
        "label": "Q1 2024",
        "data": [45, 67, 52, 39]
      },
      {
        "label": "Q2 2024",
        "data": [67, 89, 78, 56]
      }
    ]
  }
}
```

As you can see, all regions showed strong growth in Q2, with East leading at 50% growth.

## MANDATORY RULES FOR DATA RESPONSES:

1. **When user asks for data/numbers/comparisons**: You MUST output ```table or ```chart blocks
2. **DO NOT** just describe the data - SHOW it in structured format
3. **DO NOT** say "Here's a table" without the actual ```table block following
4. **DO NOT** say "I'll create a chart" without the actual ```chart block following
5. **ALWAYS** use code blocks for: sales data, statistics, comparisons, trends, metrics, reports

EXAMPLES OF CORRECT RESPONSES:
✅ User: "Show Q4 sales" → Assistant outputs: ```table with actual data
✅ User: "Compare revenue" → Assistant outputs: ```chart with actual JSON
✅ User: "List top products" → Assistant outputs: ```table with products
✅ User: "Explain authentication flow" → Assistant outputs: ```sequence diagram
✅ User: "Show RAG architecture" → Assistant outputs: ```flowchart diagram
✅ User: "Draw database schema" → Assistant outputs: ```erdiagram
✅ User: "Show user login journey" → Assistant outputs: ```journey diagram
✅ User: "Explain state machine" → Assistant outputs: ```state diagram
✅ User: "Show project timeline" → Assistant outputs: ```gantt chart

EXAMPLES OF INCORRECT RESPONSES (DO NOT DO THIS):
❌ "Here is the sales data for Q4" (WITHOUT actual ```table block)
❌ "I've created a comparison chart" (WITHOUT actual ```chart block)
❌ "The data shows..." (WITHOUT structured output)
❌ "The workflow looks like this..." (WITHOUT actual diagram block)
❌ "Here's how the API works..." (WITHOUT ```sequence block)

REMEMBER: Code blocks with data > Describing that you'll provide data
"##;

/// Parse LLM response and extract structured outputs
pub fn parse_llm_response(response: &str) -> Vec<StructuredOutput> {
    let mut outputs = Vec::new();
    let mut current_text = String::new();

    // Split by code blocks (triple backticks)
    let parts: Vec<&str> = response.split("```").collect();

    for (i, part) in parts.iter().enumerate() {
        if i % 2 == 0 {
            // Text content (outside code blocks)
            if !part.trim().is_empty() {
                current_text.push_str(part);
            }
        } else {
            // Code block content
            // Flush accumulated text first
            if !current_text.trim().is_empty() {
                outputs.push(StructuredOutput::Text {
                    content: current_text.trim().to_string()
                });
                current_text.clear();
            }

            // Determine block type and parse
            if let Some(content) = part.strip_prefix("table\n") {
                if let Some(table) = parse_markdown_table(content) {
                    outputs.push(StructuredOutput::Table {
                        headers: table.0,
                        rows: table.1,
                        caption: None,
                    });
                }
            } else if let Some(content) = part.strip_prefix("chart\n") {
                if let Ok(chart_spec) = serde_json::from_str::<ChartSpec>(content.trim()) {
                    outputs.push(StructuredOutput::Chart {
                        chart_type: chart_spec.chart_type,
                        title: chart_spec.title,
                        data: chart_spec.data,
                        description: chart_spec.description,
                    });
                }
            } else if let Some((diagram_type, content)) = parse_diagram_block(part) {
                // Mermaid diagram - extract type and content
                let mermaid = content.trim().to_string();
                // Try to extract title from first line if it's a comment
                let title = if mermaid.starts_with("%%") {
                    mermaid.lines().next()
                        .map(|l| l.trim_start_matches("%%").trim().to_string())
                        .unwrap_or_else(|| default_diagram_title(&diagram_type))
                } else {
                    default_diagram_title(&diagram_type)
                };

                outputs.push(StructuredOutput::Diagram {
                    diagram_type,
                    title,
                    mermaid,
                    description: None,
                });
            } else if let Some(content) = part.strip_prefix("form\n") {
                if let Ok(form_spec) = serde_json::from_str::<FormSpec>(content.trim()) {
                    outputs.push(StructuredOutput::Form {
                        title: form_spec.title,
                        description: form_spec.description,
                        fields: form_spec.fields,
                    });
                }
            } else if let Some(content) = part.strip_prefix("action\n") {
                // Parse system action (file ops or commands)
                if let Ok(file_action) = serde_json::from_str::<FileSystemAction>(content.trim()) {
                    outputs.push(StructuredOutput::SystemAction {
                        action: SystemActionType::FileSystem(file_action),
                    });
                } else if let Ok(cmd_action) = serde_json::from_str::<CommandAction>(content.trim()) {
                    outputs.push(StructuredOutput::SystemAction {
                        action: SystemActionType::Command(cmd_action),
                    });
                }
            }
        }
    }

    // Add remaining text
    if !current_text.trim().is_empty() {
        outputs.push(StructuredOutput::Text {
            content: current_text.trim().to_string()
        });
    }

    // If no structured outputs found, return the whole response as text
    if outputs.is_empty() {
        outputs.push(StructuredOutput::Text {
            content: response.to_string()
        });
    }

    // FALLBACK: Try to extract malformed charts (without backticks)
    // This handles cases where LLM outputs: chart { "type": "bar" ... }
    outputs = parse_malformed_charts(outputs);

    outputs
}

/// Fallback parser for malformed chart syntax (missing backticks)
/// Detects patterns like: chart { "type": "bar", ... }
fn parse_malformed_charts(outputs: Vec<StructuredOutput>) -> Vec<StructuredOutput> {
    let mut new_outputs = Vec::new();

    for output in outputs {
        if let StructuredOutput::Text { content } = output {
            let mut remaining_text = content.as_str();
            let mut text_parts = Vec::new();

            // Look for "chart {" pattern
            while let Some(chart_start) = remaining_text.find("chart {") {
                // Add text before chart
                if chart_start > 0 {
                    let before = &remaining_text[..chart_start].trim();
                    if !before.is_empty() {
                        text_parts.push(before.to_string());
                    }
                }

                // Extract JSON starting from the "{"
                let json_start = chart_start + 6; // "chart " = 6 chars
                let json_part = &remaining_text[json_start..];

                // Find matching closing brace
                if let Some(json_str) = extract_json_object(json_part) {
                    // Try to parse as chart
                    if let Ok(chart_spec) = serde_json::from_str::<ChartSpec>(&json_str) {
                        // Flush accumulated text
                        if !text_parts.is_empty() {
                            new_outputs.push(StructuredOutput::Text {
                                content: text_parts.join("\n\n")
                            });
                            text_parts.clear();
                        }

                        // Add the chart
                        new_outputs.push(StructuredOutput::Chart {
                            chart_type: chart_spec.chart_type,
                            title: chart_spec.title,
                            data: chart_spec.data,
                            description: chart_spec.description,
                        });

                        // Move past this chart
                        remaining_text = &remaining_text[json_start + json_str.len()..];
                        continue;
                    }
                }

                // If JSON parsing failed, treat "chart {" as regular text
                text_parts.push("chart {".to_string());
                remaining_text = &remaining_text[json_start..];
            }

            // Add any remaining text
            if !remaining_text.is_empty() {
                text_parts.push(remaining_text.to_string());
            }

            if !text_parts.is_empty() {
                new_outputs.push(StructuredOutput::Text {
                    content: text_parts.join("")
                });
            }
        } else {
            new_outputs.push(output);
        }
    }

    new_outputs
}

/// Extract a complete JSON object starting with "{"
fn extract_json_object(text: &str) -> Option<String> {
    let mut depth = 0;
    let mut in_string = false;
    let mut escape_next = false;
    let mut end_pos = 0;

    for (i, ch) in text.char_indices() {
        if escape_next {
            escape_next = false;
            continue;
        }

        match ch {
            '\\' => escape_next = true,
            '"' if !escape_next => in_string = !in_string,
            '{' if !in_string => depth += 1,
            '}' if !in_string => {
                depth -= 1;
                if depth == 0 {
                    end_pos = i + 1;
                    break;
                }
            }
            _ => {}
        }
    }

    if end_pos > 0 {
        Some(text[..end_pos].to_string())
    } else {
        None
    }
}

/// Helper structs for JSON parsing
#[derive(Debug, Deserialize)]
struct ChartSpec {
    #[serde(rename = "type")]
    chart_type: ChartType,
    title: String,
    data: ChartData,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FormSpec {
    title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    fields: Vec<FormField>,
}

/// Parse markdown table into headers and rows
fn parse_markdown_table(markdown: &str) -> Option<(Vec<String>, Vec<Vec<String>>)> {
    let lines: Vec<&str> = markdown.lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && l.starts_with('|'))
        .collect();

    if lines.len() < 2 {
        return None;
    }

    // Parse headers (first line)
    let headers: Vec<String> = lines[0]
        .split('|')
        .skip(1) // Skip leading empty string from "|"
        .filter_map(|s| {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        })
        .collect();

    if headers.is_empty() {
        return None;
    }

    // Skip separator line (second line with dashes)
    // Parse data rows (remaining lines)
    let rows: Vec<Vec<String>> = lines[2..]
        .iter()
        .filter_map(|line| {
            let cells: Vec<String> = line
                .split('|')
                .skip(1)
                .filter_map(|s| {
                    let trimmed = s.trim();
                    if trimmed.is_empty() {
                        None
                    } else {
                        Some(trimmed.to_string())
                    }
                })
                .collect();

            if cells.is_empty() || cells.len() != headers.len() {
                None
            } else {
                Some(cells)
            }
        })
        .collect();

    if rows.is_empty() {
        return None;
    }

    Some((headers, rows))
}

/// Parse diagram block and determine type
/// Returns (DiagramType, content) if recognized
fn parse_diagram_block(part: &str) -> Option<(DiagramType, &str)> {
    if let Some(content) = part.strip_prefix("flowchart\n") {
        Some((DiagramType::Flowchart, content))
    } else if let Some(content) = part.strip_prefix("sequence\n") {
        Some((DiagramType::Sequence, content))
    } else if let Some(content) = part.strip_prefix("class\n") {
        Some((DiagramType::Class, content))
    } else if let Some(content) = part.strip_prefix("erdiagram\n") {
        Some((DiagramType::ER, content))
    } else if let Some(content) = part.strip_prefix("er\n") {
        Some((DiagramType::ER, content))
    } else if let Some(content) = part.strip_prefix("state\n") {
        Some((DiagramType::State, content))
    } else if let Some(content) = part.strip_prefix("gantt\n") {
        Some((DiagramType::Gantt, content))
    } else if let Some(content) = part.strip_prefix("gitgraph\n") {
        Some((DiagramType::Git, content))
    } else if let Some(content) = part.strip_prefix("git\n") {
        Some((DiagramType::Git, content))
    } else if let Some(content) = part.strip_prefix("journey\n") {
        Some((DiagramType::Journey, content))
    } else {
        None
    }
}

/// Get default title for diagram type
fn default_diagram_title(diagram_type: &DiagramType) -> String {
    match diagram_type {
        DiagramType::Flowchart => "Flowchart",
        DiagramType::Sequence => "Sequence Diagram",
        DiagramType::Class => "Class Diagram",
        DiagramType::ER => "Entity Relationship Diagram",
        DiagramType::State => "State Diagram",
        DiagramType::Gantt => "Gantt Chart",
        DiagramType::Git => "Git Graph",
        DiagramType::Journey => "User Journey",
    }.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_table() {
        let markdown = r#"
| Name | Age | City |
|------|-----|------|
| Alice | 30 | NYC |
| Bob | 25 | LA |
"#;

        let result = parse_markdown_table(markdown);
        assert!(result.is_some());

        let (headers, rows) = result.unwrap();
        assert_eq!(headers, vec!["Name", "Age", "City"]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], vec!["Alice", "30", "NYC"]);
    }

    #[test]
    fn test_parse_llm_response_with_table() {
        let response = r#"Here's the data:

```table
| Product | Sales |
|---------|-------|
| A | 100 |
| B | 200 |
```

That's all."#;

        let outputs = parse_llm_response(response);
        assert_eq!(outputs.len(), 3); // text, table, text

        match &outputs[1] {
            StructuredOutput::Table { headers, rows, .. } => {
                assert_eq!(headers.len(), 2);
                assert_eq!(rows.len(), 2);
            }
            _ => panic!("Expected table output"),
        }
    }

    #[test]
    fn test_parse_llm_response_with_chart() {
        let response = r#"Sales data:

```chart
{
  "type": "bar",
  "title": "Monthly Sales",
  "data": {
    "labels": ["Jan", "Feb"],
    "datasets": [{
      "label": "2024",
      "data": [100, 200]
    }]
  }
}
```"#;

        let outputs = parse_llm_response(response);
        assert!(outputs.len() >= 2);

        match &outputs[1] {
            StructuredOutput::Chart { chart_type, title, .. } => {
                assert!(matches!(chart_type, ChartType::Bar));
                assert_eq!(title, "Monthly Sales");
            }
            _ => panic!("Expected chart output"),
        }
    }
}
