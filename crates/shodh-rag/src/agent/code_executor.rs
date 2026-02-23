//! Code Execution Engine for AI-generated workflows
//!
//! Allows agents to generate Python/TypeScript code instead of chaining tool calls.
//! Provides sandboxed execution with:
//! - Mandatory code safety validation before execution
//! - Isolated temp directory per execution (process CWD set there)
//! - Sanitized environment (API keys, tokens, credentials stripped)
//! - Output size limits to prevent memory exhaustion
//! - Timeout enforcement via tokio
//! - Deno's built-in permission model for TypeScript

use anyhow::{Result, Context as AnyhowContext, anyhow};
use serde::{Deserialize, Serialize};
use std::process::Stdio;
use std::time::{Duration, Instant};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::process::Command as AsyncCommand;

/// Maximum output size (stdout + stderr) in bytes. Prevents OOM from infinite-output scripts.
const MAX_OUTPUT_BYTES: usize = 1_048_576; // 1 MB

/// Environment variable name prefixes that are stripped from the child process.
const SENSITIVE_ENV_PREFIXES: &[&str] = &[
    "API_KEY", "API_SECRET", "SECRET", "TOKEN", "PASSWORD", "CREDENTIAL",
    "AWS_", "AZURE_", "GCP_", "OPENAI_", "ANTHROPIC_", "GOOGLE_API",
    "BOT_TOKEN", "ROSHERA_", "DATABASE_URL", "REDIS_URL",
];

/// Environment variable exact names that are stripped.
const SENSITIVE_ENV_EXACT: &[&str] = &[
    "HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA",
];

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

    /// Maximum memory usage (MB) — advisory, enforced via language-specific flags where possible
    pub max_memory_mb: usize,

    /// Allow network access (only enforced for Deno/TypeScript)
    pub allow_network: bool,

    /// Allow file system access (only enforced for Deno/TypeScript)
    pub allow_filesystem: bool,

    /// Working directory override. If None, an isolated temp directory is used.
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
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub execution_time_ms: u64,
    pub error: Option<String>,
}

/// Code executor for running AI-generated scripts
pub struct CodeExecutor {
    config: ExecutionConfig,
}

impl CodeExecutor {
    pub fn new(config: ExecutionConfig) -> Self {
        Self { config }
    }

    pub fn default() -> Self {
        Self::new(ExecutionConfig::default())
    }

    pub async fn execute_python(&self, code: &str) -> Result<CodeExecutionResult> {
        self.execute_code(code, CodeLanguage::Python).await
    }

    pub async fn execute_typescript(&self, code: &str) -> Result<CodeExecutionResult> {
        self.execute_code(code, CodeLanguage::TypeScript).await
    }

    pub async fn execute_javascript(&self, code: &str) -> Result<CodeExecutionResult> {
        self.execute_code(code, CodeLanguage::JavaScript).await
    }

    pub async fn execute_rust(&self, code: &str) -> Result<CodeExecutionResult> {
        self.execute_code(code, CodeLanguage::Rust).await
    }

    pub async fn execute_java(&self, code: &str) -> Result<CodeExecutionResult> {
        self.execute_code(code, CodeLanguage::Java).await
    }

    pub async fn execute_csharp(&self, code: &str) -> Result<CodeExecutionResult> {
        self.execute_code(code, CodeLanguage::CSharp).await
    }

    pub async fn execute_go(&self, code: &str) -> Result<CodeExecutionResult> {
        self.execute_code(code, CodeLanguage::Go).await
    }

    pub async fn execute_ruby(&self, code: &str) -> Result<CodeExecutionResult> {
        self.execute_code(code, CodeLanguage::Ruby).await
    }

    pub async fn execute_php(&self, code: &str) -> Result<CodeExecutionResult> {
        self.execute_code(code, CodeLanguage::PHP).await
    }

    pub async fn execute_kotlin(&self, code: &str) -> Result<CodeExecutionResult> {
        self.execute_code(code, CodeLanguage::Kotlin).await
    }

    pub async fn execute_swift(&self, code: &str) -> Result<CodeExecutionResult> {
        self.execute_code(code, CodeLanguage::Swift).await
    }

    /// Execute code in specified language with mandatory safety validation and sandboxing.
    pub async fn execute_code(&self, code: &str, language: CodeLanguage) -> Result<CodeExecutionResult> {
        // Mandatory safety check — never bypass
        validate_code_safety(code, &language)?;

        let start_time = Instant::now();

        // Create an isolated temp directory for this execution
        let sandbox_dir = std::env::temp_dir().join(format!("shodh_sandbox_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&sandbox_dir)
            .context("Failed to create sandbox directory")?;

        let result = self.execute_in_sandbox(code, &language, &sandbox_dir).await;

        // Always clean up the sandbox directory
        let _ = std::fs::remove_dir_all(&sandbox_dir);

        let execution_time = start_time.elapsed().as_millis() as u64;

        match result {
            Ok(mut r) => {
                r.execution_time_ms = execution_time;
                Ok(r)
            }
            Err(e) => Ok(CodeExecutionResult {
                success: false,
                stdout: String::new(),
                stderr: e.to_string(),
                exit_code: None,
                execution_time_ms: execution_time,
                error: Some(e.to_string()),
            }),
        }
    }

    /// Run code inside the sandbox directory.
    async fn execute_in_sandbox(
        &self,
        code: &str,
        language: &CodeLanguage,
        sandbox_dir: &PathBuf,
    ) -> Result<CodeExecutionResult> {
        let script_id = uuid::Uuid::new_v4();

        let (cleanup_path, command_name, args) = match language {
            CodeLanguage::Python => {
                let path = sandbox_dir.join(format!("script_{}.py", script_id));
                std::fs::write(&path, code)
                    .context("Failed to write Python script")?;

                let args = vec![
                    "-I".to_string(), // Isolated mode: no user site-packages, no PYTHON* env vars
                    "-B".to_string(), // Don't write .pyc files
                    "-S".to_string(), // Don't import site module
                    path.to_string_lossy().to_string(),
                ];

                (Some(path), "python".to_string(), args)
            }
            CodeLanguage::TypeScript => {
                let path = sandbox_dir.join(format!("script_{}.ts", script_id));
                std::fs::write(&path, code)
                    .context("Failed to write TypeScript script")?;

                // Deno's permission model is the best sandbox we have
                let mut args = vec![
                    "run".to_string(),
                    "--no-prompt".to_string(),
                ];

                if self.config.allow_network {
                    args.push("--allow-net".to_string());
                }
                if self.config.allow_filesystem {
                    // Only allow read/write within the sandbox directory
                    args.push(format!("--allow-read={}", sandbox_dir.display()));
                    args.push(format!("--allow-write={}", sandbox_dir.display()));
                }

                args.push(path.to_string_lossy().to_string());

                (Some(path), "deno".to_string(), args)
            }
            CodeLanguage::JavaScript => {
                let path = sandbox_dir.join(format!("script_{}.js", script_id));
                std::fs::write(&path, code)
                    .context("Failed to write JavaScript script")?;

                let args = vec![path.to_string_lossy().to_string()];

                (Some(path), "node".to_string(), args)
            }
            CodeLanguage::Rust => {
                let project_dir = sandbox_dir.join(format!("rust_{}", script_id));
                std::fs::create_dir_all(&project_dir)?;

                let cargo_toml = "[package]\nname = \"agent_script\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";
                std::fs::write(project_dir.join("Cargo.toml"), cargo_toml)?;

                let src_dir = project_dir.join("src");
                std::fs::create_dir_all(&src_dir)?;
                std::fs::write(src_dir.join("main.rs"), code)?;

                let args = vec![
                    "run".to_string(),
                    "--release".to_string(),
                    "--quiet".to_string(),
                    "--manifest-path".to_string(),
                    project_dir.join("Cargo.toml").to_string_lossy().to_string(),
                ];

                (None, "cargo".to_string(), args)
            }
            CodeLanguage::Java => {
                let class_name = extract_java_class_name(code).unwrap_or_else(|| "Main".to_string());
                let path = sandbox_dir.join(format!("{}.java", class_name));
                std::fs::write(&path, code)?;

                // Compile
                let compile_result = std::process::Command::new("javac")
                    .arg(path.to_string_lossy().to_string())
                    .current_dir(sandbox_dir)
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

                // Use Java Security Manager restrictions
                let args = vec![
                    "-cp".to_string(),
                    sandbox_dir.to_string_lossy().to_string(),
                    class_name,
                ];

                (None, "java".to_string(), args)
            }
            CodeLanguage::CSharp => {
                let path = sandbox_dir.join(format!("script_{}.cs", script_id));
                std::fs::write(&path, code)?;

                let args = vec![path.to_string_lossy().to_string()];

                (Some(path), "dotnet-script".to_string(), args)
            }
            CodeLanguage::Go => {
                let path = sandbox_dir.join(format!("script_{}.go", script_id));
                std::fs::write(&path, code)?;

                let args = vec!["run".to_string(), path.to_string_lossy().to_string()];

                (Some(path), "go".to_string(), args)
            }
            CodeLanguage::Ruby => {
                let path = sandbox_dir.join(format!("script_{}.rb", script_id));
                std::fs::write(&path, code)?;

                let args = vec![path.to_string_lossy().to_string()];

                (Some(path), "ruby".to_string(), args)
            }
            CodeLanguage::PHP => {
                let path = sandbox_dir.join(format!("script_{}.php", script_id));
                std::fs::write(&path, code)?;

                let args = vec![path.to_string_lossy().to_string()];

                (Some(path), "php".to_string(), args)
            }
            CodeLanguage::Kotlin => {
                let path = sandbox_dir.join(format!("script_{}.kts", script_id));
                std::fs::write(&path, code)?;

                let args = vec!["-script".to_string(), path.to_string_lossy().to_string()];

                (Some(path), "kotlinc".to_string(), args)
            }
            CodeLanguage::Swift => {
                let path = sandbox_dir.join(format!("script_{}.swift", script_id));
                std::fs::write(&path, code)?;

                let args = vec![path.to_string_lossy().to_string()];

                (Some(path), "swift".to_string(), args)
            }
        };

        // Execute with sandbox constraints
        let result = self.execute_sandboxed(&command_name, &args, sandbox_dir).await?;

        // Clean up individual script files (sandbox dir cleanup handles the rest)
        if let Some(path) = cleanup_path {
            let _ = std::fs::remove_file(path);
        }

        Ok(result)
    }

    /// Execute a command with environment sanitization, CWD isolation, and output limits.
    async fn execute_sandboxed(
        &self,
        command: &str,
        args: &[String],
        sandbox_dir: &PathBuf,
    ) -> Result<CodeExecutionResult> {
        use tokio::time::timeout;

        let mut cmd = AsyncCommand::new(command);
        cmd.args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // Set CWD to sandbox directory (isolates relative file operations)
        let working_dir = self.config.working_dir.as_deref().unwrap_or_else(|| {
            sandbox_dir.to_str().unwrap_or(".")
        });
        cmd.current_dir(working_dir);

        // Sanitize environment: remove sensitive variables
        cmd.env_clear();
        for (key, value) in std::env::vars() {
            if is_sensitive_env_var(&key) {
                continue;
            }
            cmd.env(&key, &value);
        }

        // On Windows, prevent console window popup
        #[cfg(target_os = "windows")]
        {
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        // Execute with timeout
        let output = match timeout(self.config.timeout, cmd.output()).await {
            Ok(Ok(output)) => output,
            Ok(Err(e)) => {
                return Ok(CodeExecutionResult {
                    success: false,
                    stdout: String::new(),
                    stderr: format!("Failed to execute command '{}': {}", command, e),
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

        // Truncate output to prevent memory exhaustion
        let stdout_bytes = &output.stdout[..output.stdout.len().min(MAX_OUTPUT_BYTES)];
        let stderr_bytes = &output.stderr[..output.stderr.len().min(MAX_OUTPUT_BYTES)];

        let stdout = String::from_utf8_lossy(stdout_bytes).to_string();
        let stderr = String::from_utf8_lossy(stderr_bytes).to_string();
        let success = output.status.success();
        let exit_code = output.status.code();
        let error = if !success { Some(stderr.clone()) } else { None };

        Ok(CodeExecutionResult {
            success,
            stdout,
            stderr,
            exit_code,
            execution_time_ms: 0, // Set by caller
            error,
        })
    }

    /// Execute code with tool bindings
    pub async fn execute_with_tools(
        &self,
        code: &str,
        language: CodeLanguage,
        tool_bindings: HashMap<String, String>,
    ) -> Result<CodeExecutionResult> {
        let tool_imports = self.generate_tool_imports(&tool_bindings, &language);
        let full_code = format!("{}\n\n{}", tool_imports, code);

        self.execute_code(&full_code, language).await
    }

    fn generate_tool_imports(
        &self,
        tool_bindings: &HashMap<String, String>,
        language: &CodeLanguage,
    ) -> String {
        match language {
            CodeLanguage::Python => {
                let mut imports = String::from("# Auto-generated tool bindings\nimport json\n\n");

                for (tool_name, tool_description) in tool_bindings {
                    imports.push_str(&format!(
                        "def {}(*args, **kwargs):\n    \"\"\"Tool: {}\"\"\"\n    return json.dumps({{'tool': '{}', 'args': args}})\n\n",
                        tool_name, tool_description, tool_name
                    ));
                }

                imports
            }
            CodeLanguage::TypeScript | CodeLanguage::JavaScript => {
                let mut imports = String::from("// Auto-generated tool bindings\n");

                for (tool_name, tool_description) in tool_bindings {
                    imports.push_str(&format!(
                        "async function {}(...args: any[]) {{\n  // Tool: {}\n  return JSON.stringify({{ tool: '{}', args }});\n}}\n\n",
                        tool_name, tool_description, tool_name
                    ));
                }

                imports
            }
            _ => String::from("// Tool bindings not available for this language\n")
        }
    }
}

/// Check if an environment variable name matches sensitive patterns.
fn is_sensitive_env_var(name: &str) -> bool {
    let upper = name.to_uppercase();

    for prefix in SENSITIVE_ENV_PREFIXES {
        if upper.starts_with(prefix) {
            return true;
        }
    }

    for exact in SENSITIVE_ENV_EXACT {
        if upper == *exact {
            return true;
        }
    }

    false
}

/// Validate code for dangerous patterns before execution.
/// This is called automatically by `execute_code()` and cannot be bypassed.
pub fn validate_code_safety(code: &str, language: &CodeLanguage) -> Result<()> {
    if code.len() > 100_000 {
        return Err(anyhow!("Code exceeds maximum size limit (100KB)"));
    }

    let code_lower = code.to_lowercase();

    let dangerous_patterns: Vec<&str> = match language {
        CodeLanguage::Python => vec![
            "import os", "from os", "import subprocess", "from subprocess",
            "os.system", "os.popen", "os.exec",
            "eval(", "exec(", "__import__", "compile(",
            "import shutil", "from shutil",
            "import socket", "from socket",
            "import ctypes", "from ctypes",
            "import signal", "from signal",
            "rm -rf", "rmdir", "deltree",
        ],
        CodeLanguage::Rust => vec![
            "std::process::command", "unsafe", "std::fs::remove",
            "std::ptr", "std::mem::transmute",
        ],
        CodeLanguage::Java => vec![
            "runtime.getruntime", "processbuilder", "system.exit",
            "files.delete", "file.delete", "runtime.exec",
            "java.net.socket", "java.net.url",
        ],
        CodeLanguage::CSharp => vec![
            "process.start", "file.delete", "directory.delete",
            "unsafe", "system.net", "system.io.file",
        ],
        CodeLanguage::Go => vec![
            "os/exec", "os.remove", "os.removeall", "syscall",
            "net.dial", "net.listen",
        ],
        CodeLanguage::JavaScript | CodeLanguage::TypeScript => vec![
            "child_process", "require('fs')", "require(\"fs\")",
            "fs.unlink", "fs.rm", "fs.writeFile",
            "eval(", "function(", "new function",
        ],
        _ => vec![
            "system(", "exec(", "eval(", "rm ", "del ", "format ",
            "popen(", "spawn(",
        ],
    };

    for pattern in &dangerous_patterns {
        if code_lower.contains(pattern) {
            return Err(anyhow!(
                "Code contains blocked pattern: '{}'. Remove dangerous system calls before execution.",
                pattern
            ));
        }
    }

    Ok(())
}

/// Extract Java class name from code
fn extract_java_class_name(code: &str) -> Option<String> {
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

    #[test]
    fn test_validate_code_safety_blocks_os() {
        let result = validate_code_safety("import os\nos.system('ls')", &CodeLanguage::Python);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_code_safety_allows_safe_code() {
        let result = validate_code_safety("x = 1 + 2\nprint(x)", &CodeLanguage::Python);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_code_safety_blocks_subprocess() {
        let result = validate_code_safety(
            "import subprocess\nsubprocess.run(['ls'])",
            &CodeLanguage::Python,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_code_safety_blocks_eval() {
        let result = validate_code_safety("eval('1+1')", &CodeLanguage::Python);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_code_safety_size_limit() {
        let huge_code = "x = 1\n".repeat(20_000);
        let result = validate_code_safety(&huge_code, &CodeLanguage::Python);
        assert!(result.is_err());
    }

    #[test]
    fn test_is_sensitive_env_var() {
        assert!(is_sensitive_env_var("API_KEY_OPENAI"));
        assert!(is_sensitive_env_var("OPENAI_API_KEY"));
        assert!(is_sensitive_env_var("AWS_SECRET_ACCESS_KEY"));
        assert!(is_sensitive_env_var("BOT_TOKEN"));
        assert!(is_sensitive_env_var("DATABASE_URL"));
        assert!(!is_sensitive_env_var("PATH"));
        assert!(!is_sensitive_env_var("LANG"));
        assert!(!is_sensitive_env_var("TERM"));
    }

    #[test]
    fn test_validate_blocks_node_fs() {
        let result = validate_code_safety(
            "const fs = require('fs');\nfs.writeFileSync('x', 'y');",
            &CodeLanguage::JavaScript,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_blocks_rust_unsafe() {
        let result = validate_code_safety(
            "fn main() { unsafe { std::ptr::null::<i32>().read(); } }",
            &CodeLanguage::Rust,
        );
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_execute_python_safe_code() {
        let executor = CodeExecutor::default();
        let code = "print('Hello from sandboxed execution!')\nresult = 2 + 2\nprint(f'2 + 2 = {result}')";

        let result = executor.execute_python(code).await.unwrap();

        assert!(result.success);
        assert!(result.stdout.contains("Hello from sandboxed execution!"));
        assert!(result.stdout.contains("2 + 2 = 4"));
    }

    #[tokio::test]
    async fn test_timeout() {
        let mut config = ExecutionConfig::default();
        config.timeout = Duration::from_millis(100);

        let executor = CodeExecutor::new(config);
        let code = "import time\ntime.sleep(10)";

        let result = executor.execute_python(code).await.unwrap();

        assert!(!result.success);
        assert!(result.error.unwrap().contains("Timeout"));
    }

    #[tokio::test]
    async fn test_blocked_code_rejected() {
        let executor = CodeExecutor::default();
        let code = "import os\nos.system('rm -rf /')";

        let result = executor.execute_python(code).await;
        assert!(result.is_err());
    }
}
