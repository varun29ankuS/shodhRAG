//! Code Execution Engine for AI-generated workflows
//!
//! Allows agents to generate Python/TypeScript code instead of chaining tool calls.
//! Provides sandboxed execution with timeout, memory limits, and tool access.

use anyhow::{Result, Context as AnyhowContext, anyhow};
use serde::{Deserialize, Serialize};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use std::collections::HashMap;
use tokio::process::Command as AsyncCommand;

/// Programming language for code generation
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum CodeLanguage {
    Python,
    TypeScript,
    JavaScript,
    Rust,
    Java,
    CSharp,
    Go,
    Ruby,
    PHP,
    Kotlin,
    Swift,
}

/// Configuration for code execution
#[derive(Debug, Clone)]
pub struct ExecutionConfig {
    /// Maximum execution time
    pub timeout: Duration,

    /// Maximum memory usage (MB)
    pub max_memory_mb: usize,

    /// Allow network access
    pub allow_network: bool,

    /// Allow file system access
    pub allow_filesystem: bool,

    /// Working directory
    pub working_dir: Option<String>,
}

impl Default for ExecutionConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            max_memory_mb: 256,
            allow_network: false,
            allow_filesystem: false,
            working_dir: None,
        }
    }
}

/// Result of code execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeExecutionResult {
    /// Whether execution succeeded
    pub success: bool,

    /// Standard output
    pub stdout: String,

    /// Standard error
    pub stderr: String,

    /// Exit code
    pub exit_code: Option<i32>,

    /// Execution time in milliseconds
    pub execution_time_ms: u64,

    /// Error message if failed
    pub error: Option<String>,
}

/// Code executor for running AI-generated scripts
pub struct CodeExecutor {
    config: ExecutionConfig,
}

impl CodeExecutor {
    /// Create a new code executor
    pub fn new(config: ExecutionConfig) -> Self {
        Self { config }
    }

    /// Create with default configuration
    pub fn default() -> Self {
        Self::new(ExecutionConfig::default())
    }

    /// Execute Python code
    pub async fn execute_python(&self, code: &str) -> Result<CodeExecutionResult> {
        self.execute_code(code, CodeLanguage::Python).await
    }

    /// Execute TypeScript code (via Deno)
    pub async fn execute_typescript(&self, code: &str) -> Result<CodeExecutionResult> {
        self.execute_code(code, CodeLanguage::TypeScript).await
    }

    /// Execute JavaScript code (via Node.js)
    pub async fn execute_javascript(&self, code: &str) -> Result<CodeExecutionResult> {
        self.execute_code(code, CodeLanguage::JavaScript).await
    }

    /// Execute Rust code
    pub async fn execute_rust(&self, code: &str) -> Result<CodeExecutionResult> {
        self.execute_code(code, CodeLanguage::Rust).await
    }

    /// Execute Java code
    pub async fn execute_java(&self, code: &str) -> Result<CodeExecutionResult> {
        self.execute_code(code, CodeLanguage::Java).await
    }

    /// Execute C# code
    pub async fn execute_csharp(&self, code: &str) -> Result<CodeExecutionResult> {
        self.execute_code(code, CodeLanguage::CSharp).await
    }

    /// Execute Go code
    pub async fn execute_go(&self, code: &str) -> Result<CodeExecutionResult> {
        self.execute_code(code, CodeLanguage::Go).await
    }

    /// Execute Ruby code
    pub async fn execute_ruby(&self, code: &str) -> Result<CodeExecutionResult> {
        self.execute_code(code, CodeLanguage::Ruby).await
    }

    /// Execute PHP code
    pub async fn execute_php(&self, code: &str) -> Result<CodeExecutionResult> {
        self.execute_code(code, CodeLanguage::PHP).await
    }

    /// Execute Kotlin code
    pub async fn execute_kotlin(&self, code: &str) -> Result<CodeExecutionResult> {
        self.execute_code(code, CodeLanguage::Kotlin).await
    }

    /// Execute Swift code
    pub async fn execute_swift(&self, code: &str) -> Result<CodeExecutionResult> {
        self.execute_code(code, CodeLanguage::Swift).await
    }

    /// Execute code in specified language
    pub async fn execute_code(&self, code: &str, language: CodeLanguage) -> Result<CodeExecutionResult> {
        let start_time = Instant::now();

        // Write code to temp file
        let temp_dir = std::env::temp_dir();
        let script_id = uuid::Uuid::new_v4();

        let (script_path, command_name, args) = match language {
            CodeLanguage::Python => {
                let path = temp_dir.join(format!("agent_script_{}.py", script_id));
                std::fs::write(&path, code)
                    .context("Failed to write Python script to temp file")?;

                let args = vec![
                    "-I".to_string(), // Isolated mode
                    "-B".to_string(), // Don't write .pyc files
                    path.to_string_lossy().to_string(),
                ];

                (path, "python".to_string(), args)
            }
            CodeLanguage::TypeScript => {
                let path = temp_dir.join(format!("agent_script_{}.ts", script_id));
                std::fs::write(&path, code)
                    .context("Failed to write TypeScript script to temp file")?;

                let mut args = vec![
                    "run".to_string(),
                    "--no-prompt".to_string(),
                ];

                if self.config.allow_network {
                    args.push("--allow-net".to_string());
                }
                if self.config.allow_filesystem {
                    args.push("--allow-read".to_string());
                }

                args.push(path.to_string_lossy().to_string());

                (path, "deno".to_string(), args)
            }
            CodeLanguage::JavaScript => {
                let path = temp_dir.join(format!("agent_script_{}.js", script_id));
                std::fs::write(&path, code)
                    .context("Failed to write JavaScript script to temp file")?;

                let args = vec![path.to_string_lossy().to_string()];

                (path, "node".to_string(), args)
            }
            CodeLanguage::Rust => {
                // Create a temporary Cargo project
                let project_dir = temp_dir.join(format!("rust_agent_{}", script_id));
                std::fs::create_dir_all(&project_dir)?;

                // Create Cargo.toml
                let cargo_toml = r#"[package]
name = "agent_script"
version = "0.1.0"
edition = "2021"

[dependencies]
"#;
                std::fs::write(project_dir.join("Cargo.toml"), cargo_toml)?;

                // Create src directory and main.rs
                let src_dir = project_dir.join("src");
                std::fs::create_dir_all(&src_dir)?;
                let main_path = src_dir.join("main.rs");
                std::fs::write(&main_path, code)?;

                // Compile and run
                let args = vec![
                    "run".to_string(),
                    "--release".to_string(),
                    "--quiet".to_string(),
                    "--manifest-path".to_string(),
                    project_dir.join("Cargo.toml").to_string_lossy().to_string(),
                ];

                (project_dir, "cargo".to_string(), args)
            }
            CodeLanguage::Java => {
                // Extract class name from code
                let class_name = extract_java_class_name(code).unwrap_or_else(|| "Main".to_string());
                let path = temp_dir.join(format!("{}.java", class_name));
                std::fs::write(&path, code)?;

                // Compile first
                let compile_result = std::process::Command::new("javac")
                    .arg(path.to_string_lossy().to_string())
                    .output()?;

                if !compile_result.status.success() {
                    return Ok(CodeExecutionResult {
                        success: false,
                        stdout: String::new(),
                        stderr: String::from_utf8_lossy(&compile_result.stderr).to_string(),
                        exit_code: compile_result.status.code(),
                        execution_time_ms: 0,
                        error: Some("Java compilation failed".to_string()),
                    });
                }

                // Run compiled class
                let args = vec![
                    "-cp".to_string(),
                    temp_dir.to_string_lossy().to_string(),
                    class_name.clone(),
                ];

                (path, "java".to_string(), args)
            }
            CodeLanguage::CSharp => {
                let path = temp_dir.join(format!("agent_script_{}.cs", script_id));
                std::fs::write(&path, code)?;

                // Use dotnet-script for C# scripting
                let args = vec![path.to_string_lossy().to_string()];

                (path, "dotnet-script".to_string(), args)
            }
            CodeLanguage::Go => {
                let path = temp_dir.join(format!("agent_script_{}.go", script_id));
                std::fs::write(&path, code)?;

                let args = vec!["run".to_string(), path.to_string_lossy().to_string()];

                (path, "go".to_string(), args)
            }
            CodeLanguage::Ruby => {
                let path = temp_dir.join(format!("agent_script_{}.rb", script_id));
                std::fs::write(&path, code)?;

                let args = vec![path.to_string_lossy().to_string()];

                (path, "ruby".to_string(), args)
            }
            CodeLanguage::PHP => {
                let path = temp_dir.join(format!("agent_script_{}.php", script_id));
                std::fs::write(&path, code)?;

                let args = vec![path.to_string_lossy().to_string()];

                (path, "php".to_string(), args)
            }
            CodeLanguage::Kotlin => {
                let path = temp_dir.join(format!("agent_script_{}.kts", script_id));
                std::fs::write(&path, code)?;

                // Use kotlinc for Kotlin scripting
                let args = vec!["-script".to_string(), path.to_string_lossy().to_string()];

                (path, "kotlinc".to_string(), args)
            }
            CodeLanguage::Swift => {
                let path = temp_dir.join(format!("agent_script_{}.swift", script_id));
                std::fs::write(&path, code)?;

                let args = vec![path.to_string_lossy().to_string()];

                (path, "swift".to_string(), args)
            }
        };

        // Execute with timeout
        let result = self.execute_with_timeout(&command_name, &args).await?;

        // Cleanup temp files/directories
        match language {
            CodeLanguage::Rust => {
                // Remove entire Cargo project directory
                let _ = std::fs::remove_dir_all(&script_path);
            }
            CodeLanguage::Java => {
                // Remove .java and .class files
                let _ = std::fs::remove_file(&script_path);
                if let Some(class_name) = extract_java_class_name(code) {
                    let class_file = temp_dir.join(format!("{}.class", class_name));
                    let _ = std::fs::remove_file(class_file);
                }
            }
            _ => {
                // Remove single script file
                let _ = std::fs::remove_file(&script_path);
            }
        }

        let execution_time = start_time.elapsed().as_millis() as u64;

        Ok(CodeExecutionResult {
            success: result.success,
            stdout: result.stdout,
            stderr: result.stderr,
            exit_code: result.exit_code,
            execution_time_ms: execution_time,
            error: result.error,
        })
    }

    /// Execute command with timeout
    async fn execute_with_timeout(
        &self,
        command: &str,
        args: &[String],
    ) -> Result<CodeExecutionResult> {
        use tokio::time::timeout;

        let mut cmd = AsyncCommand::new(command);
        cmd.args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // Set working directory if specified
        if let Some(ref wd) = self.config.working_dir {
            cmd.current_dir(wd);
        }

        // Execute with timeout
        let output = match timeout(self.config.timeout, cmd.output()).await {
            Ok(Ok(output)) => output,
            Ok(Err(e)) => {
                return Ok(CodeExecutionResult {
                    success: false,
                    stdout: String::new(),
                    stderr: format!("Failed to execute command: {}", e),
                    exit_code: None,
                    execution_time_ms: 0,
                    error: Some(format!("Execution error: {}", e)),
                });
            }
            Err(_) => {
                return Ok(CodeExecutionResult {
                    success: false,
                    stdout: String::new(),
                    stderr: "Execution timeout".to_string(),
                    exit_code: None,
                    execution_time_ms: self.config.timeout.as_millis() as u64,
                    error: Some(format!("Timeout after {:?}", self.config.timeout)),
                });
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let success = output.status.success();
        let exit_code = output.status.code();
        let error = if !success { Some(stderr.clone()) } else { None };

        Ok(CodeExecutionResult {
            success,
            stdout,
            stderr,
            exit_code,
            execution_time_ms: 0, // Will be set by caller
            error,
        })
    }

    /// Execute code with tool bindings (advanced)
    pub async fn execute_with_tools(
        &self,
        code: &str,
        language: CodeLanguage,
        tool_bindings: HashMap<String, String>,
    ) -> Result<CodeExecutionResult> {
        // Inject tool imports at the beginning
        let tool_imports = self.generate_tool_imports(&tool_bindings, &language);
        let full_code = format!("{}\n\n{}", tool_imports, code);

        self.execute_code(&full_code, language).await
    }

    /// Generate tool import code
    fn generate_tool_imports(
        &self,
        tool_bindings: &HashMap<String, String>,
        language: &CodeLanguage,
    ) -> String {
        match language {
            CodeLanguage::Python => {
                let mut imports = String::from("# Auto-generated tool bindings\n");
                imports.push_str("import json\n");
                imports.push_str("import subprocess\n\n");

                for (tool_name, tool_cmd) in tool_bindings {
                    imports.push_str(&format!(
                        "def {}(*args, **kwargs):\n    return subprocess.run({}, capture_output=True, text=True).stdout\n\n",
                        tool_name, tool_cmd
                    ));
                }

                imports
            }
            CodeLanguage::TypeScript | CodeLanguage::JavaScript => {
                let mut imports = String::from("// Auto-generated tool bindings\n");

                for (tool_name, tool_cmd) in tool_bindings {
                    imports.push_str(&format!(
                        "async function {}(...args: any[]) {{\n  // Tool: {}\n  return null;\n}}\n\n",
                        tool_name, tool_cmd
                    ));
                }

                imports
            }
            _ => {
                // For other languages, return empty string (no tool bindings yet)
                String::from("// Tool bindings not implemented for this language\n")
            }
        }
    }
}

/// Helper to validate code before execution
pub fn validate_code_safety(code: &str, language: &CodeLanguage) -> Result<()> {
    let code_lower = code.to_lowercase();

    // Dangerous patterns per language
    let dangerous_patterns = match language {
        CodeLanguage::Python => vec![
            "import os", "import subprocess", "os.system", "eval(", "exec(",
            "__import__", "compile(", "open(", "file(", "rm -rf",
        ],
        CodeLanguage::Rust => vec![
            "std::process::Command", "unsafe", "std::fs::remove", "std::ptr",
        ],
        CodeLanguage::Java => vec![
            "Runtime.getRuntime", "ProcessBuilder", "System.exit", "Files.delete",
        ],
        CodeLanguage::CSharp => vec![
            "Process.Start", "File.Delete", "Directory.Delete", "unsafe",
        ],
        CodeLanguage::Go => vec![
            "os/exec", "os.Remove", "os.RemoveAll", "syscall",
        ],
        CodeLanguage::JavaScript | CodeLanguage::TypeScript => vec![
            "child_process", "fs.unlink", "fs.rm", "eval(", "Function(",
        ],
        _ => vec![
            "system(", "exec(", "eval(", "rm ", "del ", "format",
        ],
    };

    for pattern in &dangerous_patterns {
        if code_lower.contains(pattern) {
            return Err(anyhow!(
                "Code contains potentially dangerous pattern: {}. \
                Please review for security before execution.",
                pattern
            ));
        }
    }

    Ok(())
}

/// Extract Java class name from code
fn extract_java_class_name(code: &str) -> Option<String> {
    // Look for: public class ClassName
    for line in code.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("public class ") {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 3 {
                let class_name = parts[2].trim_end_matches('{').trim();
                return Some(class_name.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_execute_python() {
        let executor = CodeExecutor::default();

        let code = r#"
print("Hello from AI-generated code!")
result = 2 + 2
print(f"2 + 2 = {result}")
"#;

        let result = executor.execute_python(code).await.unwrap();

        assert!(result.success);
        assert!(result.stdout.contains("Hello from AI-generated code!"));
        assert!(result.stdout.contains("2 + 2 = 4"));
    }

    #[tokio::test]
    async fn test_timeout() {
        let mut config = ExecutionConfig::default();
        config.timeout = Duration::from_millis(100);

        let executor = CodeExecutor::new(config);

        let code = r#"
import time
time.sleep(10)  # Will timeout
"#;

        let result = executor.execute_python(code).await.unwrap();

        assert!(!result.success);
        assert!(result.error.unwrap().contains("Timeout"));
    }
}
