//! Code Generation Agent - Generates and executes Python code instead of tool chaining
//!
//! This agent follows Anthropic's MCP approach: instead of making multiple LLM calls
//! to chain tools together, it generates a single Python script that accomplishes the
//! entire workflow, then executes it in a sandbox.
//!
//! Benefits over traditional tool-chaining:
//! - ~98% fewer tokens (1 LLM call vs 3-5)
//! - Deterministic execution (code is repeatable)
//! - Debuggable workflows (inspect the generated code)
//! - Natural composition (functions call functions)

use super::code_executor::{CodeExecutor, CodeLanguage, ExecutionConfig, CodeExecutionResult};
use super::tools::ToolRegistry;
use anyhow::{Result, Context as AnyhowContext};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::collections::HashMap;

/// Code generation agent configuration
#[derive(Debug, Clone)]
pub struct CodeGenAgent {
    /// Name of the agent
    pub name: String,

    /// System prompt for code generation
    pub system_prompt: String,

    /// Available tools/functions
    pub available_tools: Vec<ToolSpec>,

    /// Code executor
    pub executor: CodeExecutor,

    /// Target language
    pub language: CodeLanguage,
}

/// Specification for a tool that can be called from generated code
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    /// Tool name
    pub name: String,

    /// Function signature
    pub signature: String,

    /// Description of what it does
    pub description: String,

    /// Example usage
    pub example: String,
}

impl CodeGenAgent {
    /// Create a new code generation agent
    pub fn new(name: String, available_tools: Vec<ToolSpec>) -> Self {
        let system_prompt = Self::build_system_prompt(&available_tools);

        Self {
            name,
            system_prompt,
            available_tools,
            executor: CodeExecutor::default(),
            language: CodeLanguage::Python,
        }
    }

    /// Build the system prompt that instructs LLM to generate code
    fn build_system_prompt(tools: &[ToolSpec]) -> String {
        let mut prompt = String::from(
            "You are an expert Python code generator. Your task is to write complete, \
            executable Python code to solve user requests.\n\n\
            IMPORTANT: Do NOT explain your code. Do NOT use markdown. Write ONLY raw Python code.\n\n\
            Available functions:\n\n"
        );

        // Add tool signatures
        prompt.push_str("```python\n");
        for tool in tools {
            prompt.push_str(&format!("def {}:\n", tool.signature));
            prompt.push_str(&format!("    \"\"\"{}\"\"\"\n", tool.description));
            prompt.push_str("    pass\n\n");
        }
        prompt.push_str("```\n\n");

        // Add examples
        prompt.push_str("Example workflows:\n\n");
        for tool in tools {
            if !tool.example.is_empty() {
                prompt.push_str(&format!("```python\n{}\n```\n\n", tool.example));
            }
        }

        prompt.push_str(
            "Now, write clean Python code to solve the user's task. \
            Your code will be executed automatically. Include print() statements \
            to show results.\n\n\
            Remember:\n\
            - Write ONLY Python code (no explanations, no markdown)\n\
            - Use the provided functions\n\
            - Handle errors gracefully\n\
            - Print results clearly\n"
        );

        prompt
    }

    /// Generate and execute code for a user query
    pub async fn execute_query(
        &self,
        query: &str,
        llm_generate_fn: impl Fn(&str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String>> + Send>>,
    ) -> Result<CodeExecutionResult> {
        // Step 1: Generate code via LLM
        let prompt = format!("{}\n\nUser request: {}\n\nGenerate Python code:", self.system_prompt, query);

        let generated_code = llm_generate_fn(&prompt).await
            .context("Failed to generate code via LLM")?;

        // Clean up the generated code (remove markdown fences if present)
        let clean_code = self.clean_generated_code(&generated_code);

        tracing::debug!(code = %clean_code, "Generated code");

        // Step 2: Validate code safety
        super::code_executor::validate_code_safety(&clean_code, &self.language)?;

        // Step 3: Execute code
        let result = self.executor.execute_python(&clean_code).await
            .context("Failed to execute generated code")?;

        tracing::info!(success = result.success, "Execution result");
        tracing::debug!(output = %result.stdout, "Execution output");

        if !result.stderr.is_empty() {
            tracing::warn!(stderr = %result.stderr, "Execution errors");
        }

        Ok(result)
    }

    /// Clean generated code (remove markdown, comments, etc.)
    fn clean_generated_code(&self, raw_code: &str) -> String {
        let mut lines: Vec<&str> = raw_code.lines().collect();

        // Remove markdown code fences
        if lines.first().map(|l| l.trim().starts_with("```")).unwrap_or(false) {
            lines.remove(0);
        }
        if lines.last().map(|l| l.trim().starts_with("```")).unwrap_or(false) {
            lines.pop();
        }

        // Remove leading/trailing whitespace
        lines.join("\n").trim().to_string()
    }
}

/// Create a code generation agent with RAG tools
pub fn create_rag_code_agent() -> CodeGenAgent {
    let tools = vec![
        ToolSpec {
            name: "search_documents".to_string(),
            signature: "search_documents(query: str, limit: int = 5) -> List[Dict]".to_string(),
            description: "Search the knowledge base for relevant documents".to_string(),
            example: r#"results = search_documents("machine learning", limit=10)
for doc in results:
    print(f"- {doc['title']}: {doc['snippet']}")"#.to_string(),
        },
        ToolSpec {
            name: "summarize_text".to_string(),
            signature: "summarize_text(text: str, max_words: int = 100) -> str".to_string(),
            description: "Summarize a long text into concise form".to_string(),
            example: r#"long_article = "..."
summary = summarize_text(long_article, max_words=50)
print(f"Summary: {summary}")"#.to_string(),
        },
        ToolSpec {
            name: "extract_entities".to_string(),
            signature: "extract_entities(text: str) -> List[str]".to_string(),
            description: "Extract named entities (people, places, organizations) from text".to_string(),
            example: r#"text = "Apple Inc. was founded by Steve Jobs in Cupertino."
entities = extract_entities(text)
print(f"Found: {entities}")"#.to_string(),
        },
    ];

    CodeGenAgent::new("RAG Code Generator".to_string(), tools)
}

/// Create a code generation agent with workflow automation tools
pub fn create_workflow_code_agent() -> CodeGenAgent {
    let tools = vec![
        ToolSpec {
            name: "search_documents".to_string(),
            signature: "search_documents(query: str, limit: int = 5) -> List[Dict]".to_string(),
            description: "Search knowledge base".to_string(),
            example: "results = search_documents('emails', 10)".to_string(),
        },
        ToolSpec {
            name: "summarize_text".to_string(),
            signature: "summarize_text(text: str, max_words: int = 100) -> str".to_string(),
            description: "Summarize text".to_string(),
            example: "summary = summarize_text(long_text, 50)".to_string(),
        },
        ToolSpec {
            name: "send_notification".to_string(),
            signature: "send_notification(title: str, message: str) -> bool".to_string(),
            description: "Send a notification to the user".to_string(),
            example: "send_notification('Daily Brief', summary)".to_string(),
        },
    ];

    CodeGenAgent::new("Workflow Automator".to_string(), tools)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_system_prompt_generation() {
        let tools = vec![
            ToolSpec {
                name: "search".to_string(),
                signature: "search(query: str) -> List[str]".to_string(),
                description: "Search documents".to_string(),
                example: "results = search('test')".to_string(),
            },
        ];

        let agent = CodeGenAgent::new("Test".to_string(), tools);

        assert!(agent.system_prompt.contains("search(query: str)"));
        assert!(agent.system_prompt.contains("Search documents"));
    }

    #[test]
    fn test_code_cleaning() {
        let agent = create_rag_code_agent();

        let raw = r#"```python
print("Hello")
```"#;

        let clean = agent.clean_generated_code(raw);
        assert_eq!(clean, "print(\"Hello\")");
    }
}
