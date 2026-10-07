//! The pinned `omp` sidecar: binary resolution and integrity, isolated
//! layout, launch arguments and environment, process spawn, and download.
//!
//! The binary is never trusted by path alone: its SHA-256 is checked before
//! every launch. On Windows x64 the hash is pinned in this file. On other
//! platforms [`fetch_omp`] verifies the download against the release's
//! `SHA256SUMS.txt` (over HTTPS) and records the hash next to the binary.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};

use super::code_mode::{
    code_launch_args, code_overlay_config, CodeFolder, CODE_ROOT_ENV, EDIT_VARIANT_ENV, GUARD_HOOK,
};
use super::error::HarnessError;
use super::model::{EnvValue, OmpModel};

/// Pinned omp release.
pub const OMP_VERSION: &str = "18.4.10";

/// SHA-256 of `omp-windows-x64.exe` from the v18.4.10 release.
pub const OMP_WINDOWS_X64_SHA256: &str =
    "7232c209641f0cad7e20bdb3a074cdb2fb31ae2aa73d42c491c705d28e0d3895";

/// Overrides the binary location (still hash-verified).
pub const OMP_PATH_ENV: &str = "SHODH_OMP_PATH";

const RELEASE_BASE_URL: &str = "https://github.com/can1357/oh-my-pi/releases/download/v18.4.10";
const SHA256SUMS_ASSET: &str = "SHA256SUMS.txt";
const MAX_DOWNLOAD_BYTES: u64 = 512 * 1024 * 1024;

/// Launch flags (plan, Global Constraints). Built-in tools, extensions,
/// skills, rules, LSP and PTY are off; host tools are the only tools.
pub const BASE_FLAGS: [&str; 12] = [
    "--mode",
    "rpc",
    "--no-ui",
    "--no-tools",
    "--no-extensions",
    "--no-skills",
    "--no-rules",
    "--no-lsp",
    "--no-pty",
    "--no-session",
    "--thinking",
    "off",
];

/// Release asset for the platform this binary was built for.
pub fn release_asset_name() -> Result<&'static str, HarnessError> {
    let asset = if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        "omp-windows-x64.exe"
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "omp-linux-x64"
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        "omp-linux-arm64"
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "omp-darwin-arm64"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "omp-darwin-x64"
    } else {
        return Err(HarnessError::UnsupportedPlatform(format!(
            "{}-{}",
            std::env::consts::OS,
            std::env::consts::ARCH
        )));
    };
    Ok(asset)
}

/// The hash pinned in source for this platform, if any.
pub fn pinned_hash() -> Option<&'static str> {
    if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        Some(OMP_WINDOWS_X64_SHA256)
    } else {
        None
    }
}

/// `omp-<version>[.exe]`.
pub fn binary_file_name() -> String {
    if cfg!(windows) {
        format!("omp-{OMP_VERSION}.exe")
    } else {
        format!("omp-{OMP_VERSION}")
    }
}

/// `<app_data>/bin/omp-<version>[.exe]`.
pub fn default_binary_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("bin").join(binary_file_name())
}

/// `SHODH_OMP_PATH` if set, else the default location.
pub fn resolve_binary_path(app_data_dir: &Path) -> PathBuf {
    std::env::var_os(OMP_PATH_ENV)
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| default_binary_path(app_data_dir))
}

fn recorded_hash_path(binary: &Path) -> PathBuf {
    let mut name = binary.as_os_str().to_owned();
    name.push(".sha256");
    PathBuf::from(name)
}

/// The hash the binary must have: pinned on Windows x64, otherwise the one
/// recorded by [`fetch_omp`] after verifying against the release checksums.
pub fn expected_hash(binary: &Path) -> Result<String, HarnessError> {
    if let Some(pinned) = pinned_hash() {
        return Ok(pinned.to_string());
    }
    let recorded = std::fs::read_to_string(recorded_hash_path(binary))
        .map_err(|_| HarnessError::NoPinnedHash(std::env::consts::OS.to_string()))?;
    let recorded = recorded.trim().to_ascii_lowercase();
    if recorded.len() == 64 && recorded.chars().all(|c| c.is_ascii_hexdigit()) {
        Ok(recorded)
    } else {
        Err(HarnessError::NoPinnedHash(std::env::consts::OS.to_string()))
    }
}

/// SHA-256 of a file, computed off the async runtime.
pub async fn sha256_file(path: &Path) -> Result<String, HarnessError> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || -> Result<String, std::io::Error> {
        use std::io::Read;
        let mut file = std::fs::File::open(&path)?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        Ok(hex::encode(hasher.finalize()))
    })
    .await
    .map_err(|e| HarnessError::Spawn(format!("hashing task failed: {e}")))?
    .map_err(HarnessError::Io)
}

/// Check the binary exists and matches its expected hash.
pub async fn verify_binary(binary: &Path) -> Result<(), HarnessError> {
    if !binary.is_file() {
        return Err(HarnessError::BinaryMissing {
            path: binary.to_path_buf(),
            version: OMP_VERSION,
        });
    }
    let expected = expected_hash(binary)?;
    let actual = sha256_file(binary).await?;
    if actual.eq_ignore_ascii_case(&expected) {
        Ok(())
    } else {
        Err(HarnessError::HashMismatch {
            path: binary.to_path_buf(),
            expected,
            actual,
        })
    }
}

/// Isolated directories for omp under the app data dir.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OmpLayout {
    pub root: PathBuf,
    /// Used as HOME/USERPROFILE so omp never reads the user's own config.
    pub home: PathBuf,
    pub agent_dir: PathBuf,
    pub temp: PathBuf,
    pub overlay: PathBuf,
    pub sessions: PathBuf,
    /// Code mode's overlay and guard hook (outside every code folder).
    pub code_overlay: PathBuf,
    pub guard_hook: PathBuf,
}

impl OmpLayout {
    pub fn new(app_data_dir: &Path) -> Self {
        let root = app_data_dir.join("omp");
        let home = root.join("home");
        let code = root.join("code");
        Self {
            agent_dir: home.join(".omp").join("agent"),
            temp: root.join("tmp"),
            overlay: root.join("overlay.yml"),
            sessions: root.join("sessions"),
            code_overlay: code.join("overlay.yml"),
            guard_hook: code.join("guard.js"),
            home,
            root,
        }
    }

    /// Write Code mode's overlay and guard hook. Files whose content is
    /// already current are left alone (another Code session may be loading
    /// them).
    pub fn prepare_code(&self) -> Result<(), HarnessError> {
        let overlay = serde_json::to_string_pretty(&code_overlay_config())?;
        write_if_changed(&self.code_overlay, &overlay)?;
        write_if_changed(&self.guard_hook, GUARD_HOOK)?;
        Ok(())
    }

    /// Create the directories and (re)write the overlay config.
    pub fn prepare(&self) -> Result<(), HarnessError> {
        for dir in [&self.home, &self.agent_dir, &self.temp, &self.sessions] {
            std::fs::create_dir_all(dir)?;
        }
        let overlay = serde_json::to_string_pretty(&overlay_config())?;
        let tmp = self.overlay.with_extension("yml.tmp");
        std::fs::write(&tmp, overlay)?;
        std::fs::rename(&tmp, &self.overlay)?;
        Ok(())
    }

    /// An empty working directory for one session.
    pub fn session_dir(&self, session_id: &str) -> Result<PathBuf, HarnessError> {
        let safe: String = session_id
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect();
        let dir = self
            .sessions
            .join(if safe.is_empty() { "session" } else { &safe });
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
        std::fs::create_dir_all(&dir)?;
        Ok(dir)
    }
}

fn write_if_changed(path: &Path, content: &str) -> Result<(), HarnessError> {
    if std::fs::read_to_string(path).is_ok_and(|current| current == content) {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4().simple()));
    std::fs::write(&tmp, content)?;
    if let Err(e) = std::fs::rename(&tmp, path) {
        // Best effort: the temporary copy is useless.
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(())
}

/// The `--config` overlay. YAML is a superset of JSON, so it is written as
/// JSON. Keys follow the plan's Global Constraints plus every discovery and
/// background-feature toggle observed in omp's `config list`.
pub fn overlay_config() -> Value {
    json!({
        "providers": { "cacheWarming": "off" },
        "retry": { "fallbackChains": { "judge": [] } },
        "memory": { "backend": "off" },
        "memories": { "enabled": false },
        "advisor": { "enabled": false },
        "autolearn": { "enabled": false },
        "lsp": { "enabled": false },
        "marketplace": { "autoUpdate": "off" },
        "dev": { "autoqa": false, "autoqaConsent": "denied" },
        "skills": {
            "enabled": false,
            "enableCodexUser": false,
            "enableClaudeUser": false,
            "enableClaudeProject": false,
            "enablePiUser": false,
            "enablePiProject": false,
            "enableAgentsUser": false,
            "enableAgentsProject": false,
            "enableSkillCommands": false
        },
        "commands": {
            "enableClaudeUser": false,
            "enableClaudeProject": false,
            "enableOpencodeUser": false,
            "enableOpencodeProject": false
        }
    })
}

/// Full argument list for one session.
pub fn launch_args(model_arg: &str, overlay: &Path, system_prompt: &str) -> Vec<String> {
    let mut args: Vec<String> = BASE_FLAGS.iter().map(|s| s.to_string()).collect();
    args.push(format!("--model={model_arg}"));
    args.push(format!("--config={}", overlay.display()));
    args.push(format!("--system-prompt={system_prompt}"));
    args
}

const PASSTHROUGH_VARS: [&str; 18] = [
    // Windows process essentials.
    "SystemRoot",
    "windir",
    "SystemDrive",
    "PATHEXT",
    "COMSPEC",
    "NUMBER_OF_PROCESSORS",
    "PROCESSOR_ARCHITECTURE",
    "OS",
    // Shared.
    "PATH",
    "LANG",
    "LC_ALL",
    "TZ",
    // Corporate proxies, so the configured LLM endpoint stays reachable.
    "HTTPS_PROXY",
    "HTTP_PROXY",
    "NO_PROXY",
    "https_proxy",
    "http_proxy",
    "no_proxy",
];

/// The child's complete environment. Everything else is cleared, so provider
/// keys and tokens in the user's environment never reach omp.
pub fn child_env(
    layout: &OmpLayout,
    model: &OmpModel,
    parent: impl Fn(&str) -> Option<String>,
) -> Vec<(String, EnvValue)> {
    let mut env: Vec<(String, EnvValue)> = PASSTHROUGH_VARS
        .iter()
        .filter_map(|name| parent(name).map(|v| (name.to_string(), EnvValue::Plain(v))))
        .collect();
    let home = layout.home.display().to_string();
    let path = |p: PathBuf| EnvValue::Plain(p.display().to_string());
    env.extend([
        ("HOME".to_string(), EnvValue::Plain(home.clone())),
        ("USERPROFILE".to_string(), EnvValue::Plain(home)),
        (
            "APPDATA".to_string(),
            path(layout.home.join("AppData").join("Roaming")),
        ),
        (
            "LOCALAPPDATA".to_string(),
            path(layout.home.join("AppData").join("Local")),
        ),
        (
            "XDG_CONFIG_HOME".to_string(),
            path(layout.home.join(".config")),
        ),
        (
            "XDG_DATA_HOME".to_string(),
            path(layout.home.join(".local").join("share")),
        ),
        (
            "XDG_STATE_HOME".to_string(),
            path(layout.home.join(".local").join("state")),
        ),
        (
            "XDG_CACHE_HOME".to_string(),
            path(layout.home.join(".cache")),
        ),
        ("TEMP".to_string(), path(layout.temp.clone())),
        ("TMP".to_string(), path(layout.temp.clone())),
        ("TMPDIR".to_string(), path(layout.temp.clone())),
        (
            "PI_CODING_AGENT_DIR".to_string(),
            path(layout.agent_dir.clone()),
        ),
        ("PI_AUTO_QA".to_string(), EnvValue::Plain("0".into())),
        ("PI_NO_TITLE".to_string(), EnvValue::Plain("1".into())),
        (
            "OTEL_SDK_DISABLED".to_string(),
            EnvValue::Plain("true".into()),
        ),
        ("NO_COLOR".to_string(), EnvValue::Plain("1".into())),
    ]);
    env.extend(model.env.iter().cloned());
    env
}

/// Everything needed to start one omp process.
#[derive(Debug, Clone)]
pub struct LaunchSpec {
    pub binary: PathBuf,
    pub layout: OmpLayout,
    pub model: OmpModel,
    pub system_prompt: String,
    pub session_id: String,
    /// Code mode: omp's coding tools, confined to this folder. `None` is a
    /// Research session (host tools only).
    pub code: Option<CodeFolder>,
}

impl LaunchSpec {
    /// A loggable description: no environment values, no prompt text.
    pub fn describe(&self) -> String {
        let env_names: Vec<&str> = self.model.env.iter().map(|(k, _)| k.as_str()).collect();
        let mode = match &self.code {
            Some(folder) => format!("code in {}", folder.display()),
            None => "research".to_string(),
        };
        format!(
            "omp {} at {} (model {}, provider env {:?}, session {}, {mode})",
            OMP_VERSION,
            self.binary.display(),
            self.model.model_arg,
            env_names,
            self.session_id
        )
    }

    /// The process's arguments and working directory. A Code session works in
    /// its folder; a Research session gets an empty one of its own.
    fn command_line(&self) -> Result<(Vec<String>, PathBuf), HarnessError> {
        match &self.code {
            None => Ok((
                launch_args(
                    &self.model.model_arg,
                    &self.layout.overlay,
                    &self.system_prompt,
                ),
                self.layout.session_dir(&self.session_id)?,
            )),
            Some(folder) => {
                self.layout.prepare_code()?;
                Ok((
                    code_launch_args(
                        &self.model.model_arg,
                        &self.layout.overlay,
                        &self.layout.code_overlay,
                        &self.layout.guard_hook,
                        &self.system_prompt,
                    ),
                    folder.root().to_path_buf(),
                ))
            }
        }
    }

    /// The child's environment: [`child_env`], plus the guard's folder and
    /// the edit format in Code mode.
    fn environment(&self) -> Vec<(String, EnvValue)> {
        let mut env = child_env(&self.layout, &self.model, |n| std::env::var(n).ok());
        if let Some(folder) = &self.code {
            env.push((CODE_ROOT_ENV.to_string(), EnvValue::Plain(folder.display())));
            env.push((
                EDIT_VARIANT_ENV.0.to_string(),
                EnvValue::Plain(EDIT_VARIANT_ENV.1.to_string()),
            ));
        }
        env
    }
}

/// A running omp process with its pipes.
pub struct SidecarProcess {
    pub child: Child,
    pub stdin: ChildStdin,
    pub stdout: ChildStdout,
    pub stderr: ChildStderr,
}

/// Verify the binary, prepare the isolated layout and start omp.
pub async fn spawn(spec: &LaunchSpec) -> Result<SidecarProcess, HarnessError> {
    let started = std::time::Instant::now();
    verify_binary(&spec.binary).await?;
    let verify_ms = started.elapsed().as_millis();
    spec.layout.prepare()?;
    let (args, cwd) = spec.command_line()?;

    let mut command = Command::new(&spec.binary);
    command
        .args(args)
        .env_clear()
        .current_dir(&cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for (name, value) in spec.environment() {
        command.env(name, value.as_str());
    }
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    // Never log `command` itself: its Debug output includes the environment.
    tracing::info!(target: "shodh::harness", verify_ms, "starting {}", spec.describe());
    let mut child = command
        .spawn()
        .map_err(|e| HarnessError::Spawn(format!("{}: {e}", spec.binary.display())))?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| HarnessError::Spawn("stdin pipe unavailable".into()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| HarnessError::Spawn("stdout pipe unavailable".into()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| HarnessError::Spawn("stderr pipe unavailable".into()))?;
    Ok(SidecarProcess {
        child,
        stdin,
        stdout,
        stderr,
    })
}

/// Parse a `SHA256SUMS.txt` body and return the hash listed for `asset`.
pub fn hash_from_sums(sums: &str, asset: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let hash = parts.next()?;
        let name = parts.next()?.trim_start_matches('*');
        (name == asset && hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()))
            .then(|| hash.to_ascii_lowercase())
    })
}

/// A verified runtime installed by [`fetch_omp`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledRuntime {
    pub path: PathBuf,
    /// SHA-256 the download was verified against.
    pub sha256: String,
}

/// Download progress: bytes received and, when the server reports it, the
/// total size.
pub type FetchProgress = dyn Fn(u64, Option<u64>) + Send + Sync;

/// Download the pinned omp release for this platform into
/// `<app_data>/bin`, verify its SHA-256 and move it into place atomically.
/// `progress` is called as bytes arrive.
pub async fn fetch_omp(
    app_data_dir: &Path,
    progress: &FetchProgress,
) -> Result<InstalledRuntime, HarnessError> {
    let asset = release_asset_name()?;
    let target = default_binary_path(app_data_dir);
    let dir = target
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| app_data_dir.to_path_buf());
    tokio::fs::create_dir_all(&dir).await?;

    let client = reqwest::Client::builder()
        .https_only(true)
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(15 * 60))
        .user_agent(concat!("shodh/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| HarnessError::Download(e.to_string()))?;

    let expected = match pinned_hash() {
        Some(hash) => hash.to_string(),
        None => {
            let url = format!("{RELEASE_BASE_URL}/{SHA256SUMS_ASSET}");
            let sums = client
                .get(&url)
                .send()
                .await
                .and_then(reqwest::Response::error_for_status)
                .map_err(|e| HarnessError::Download(format!("{url}: {e}")))?
                .text()
                .await
                .map_err(|e| HarnessError::Download(format!("{url}: {e}")))?;
            hash_from_sums(&sums, asset).ok_or_else(|| {
                HarnessError::UnsupportedPlatform(format!(
                    "the omp {OMP_VERSION} release has no {asset}"
                ))
            })?
        }
    };

    let url = format!("{RELEASE_BASE_URL}/{asset}");
    let mut response = client
        .get(&url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| HarnessError::Download(format!("{url}: {e}")))?;
    let total = response.content_length();
    if total.unwrap_or(0) > MAX_DOWNLOAD_BYTES {
        return Err(HarnessError::Download(format!(
            "{url}: the file is larger than expected"
        )));
    }

    let temp = dir.join(format!(
        ".{}.{}.part",
        binary_file_name(),
        uuid::Uuid::new_v4()
    ));
    let result = async {
        let mut file = tokio::fs::File::create(&temp).await?;
        let mut hasher = Sha256::new();
        let mut written: u64 = 0;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| HarnessError::Download(format!("{url}: {e}")))?
        {
            written += chunk.len() as u64;
            if written > MAX_DOWNLOAD_BYTES {
                return Err(HarnessError::Download(format!(
                    "{url}: the file is larger than expected"
                )));
            }
            hasher.update(&chunk);
            file.write_all(&chunk).await?;
            progress(written, total);
        }
        file.flush().await?;
        file.sync_all().await?;
        drop(file);
        let actual = hex::encode(hasher.finalize());
        if !actual.eq_ignore_ascii_case(&expected) {
            return Err(HarnessError::HashMismatch {
                path: PathBuf::from(&url),
                expected: expected.clone(),
                actual,
            });
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o755)).await?;
        }
        if pinned_hash().is_none() {
            let record = recorded_hash_path(&target);
            let record_tmp = dir.join(format!(".{}.sha256.part", binary_file_name()));
            tokio::fs::write(&record_tmp, format!("{expected}\n")).await?;
            tokio::fs::rename(&record_tmp, &record).await?;
        }
        tokio::fs::rename(&temp, &target).await?;
        Ok(())
    }
    .await;

    if result.is_err() {
        // Best effort: the partial file is useless.
        let _ = tokio::fs::remove_file(&temp).await;
    }
    result?;
    tracing::info!(target: "shodh::harness", "installed omp {OMP_VERSION} at {}", target.display());
    Ok(InstalledRuntime {
        path: target,
        sha256: expected,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::model::Secret;

    fn model() -> OmpModel {
        OmpModel {
            model_arg: "anthropic/claude-haiku-4-5".into(),
            provider_label: "Anthropic",
            is_local: false,
            env: vec![(
                "ANTHROPIC_API_KEY".into(),
                EnvValue::Secret(Secret::new("sk-ant-secret")),
            )],
            warning: None,
        }
    }

    #[test]
    fn launch_args_carry_the_pinned_flags() {
        let args = launch_args(
            "openai/gpt-5-mini",
            Path::new("/data/omp/overlay.yml"),
            "Be brief.",
        );
        assert_eq!(&args[..12], &BASE_FLAGS.map(String::from));
        assert!(args.contains(&"--no-tools".to_string()));
        assert!(!args.iter().any(|a| a == "--tools"));
        assert!(args.contains(&"--model=openai/gpt-5-mini".to_string()));
        assert!(args.contains(&"--config=/data/omp/overlay.yml".to_string()));
        assert!(args.contains(&"--system-prompt=Be brief.".to_string()));
    }

    #[test]
    fn child_env_is_isolated_and_whitelisted() {
        let layout = OmpLayout::new(Path::new("/data"));
        let env = child_env(&layout, &model(), |name| match name {
            "PATH" => Some("/usr/bin".into()),
            "OPENAI_API_KEY" => Some("leak".into()),
            _ => None,
        });
        let get = |k: &str| {
            env.iter()
                .find(|(n, _)| n == k)
                .map(|(_, v)| v.as_str().to_string())
        };
        assert_eq!(get("PATH").as_deref(), Some("/usr/bin"));
        assert_eq!(get("OPENAI_API_KEY"), None);
        assert_eq!(get("HOME"), Some(layout.home.display().to_string()));
        assert_eq!(get("USERPROFILE"), Some(layout.home.display().to_string()));
        assert_eq!(get("PI_AUTO_QA").as_deref(), Some("0"));
        assert_eq!(get("OTEL_SDK_DISABLED").as_deref(), Some("true"));
        assert_eq!(get("ANTHROPIC_API_KEY").as_deref(), Some("sk-ant-secret"));
        assert!(!format!("{env:?}").contains("sk-ant-secret"));
    }

    #[test]
    fn describe_never_contains_secrets() {
        let spec = LaunchSpec {
            binary: PathBuf::from("/data/bin/omp"),
            layout: OmpLayout::new(Path::new("/data")),
            model: model(),
            system_prompt: "private instructions".into(),
            session_id: "s1".into(),
            code: None,
        };
        let text = spec.describe();
        assert!(text.contains("ANTHROPIC_API_KEY"));
        assert!(!text.contains("sk-ant-secret"));
        assert!(!text.contains("private instructions"));
    }

    fn spec(data: &Path, code: Option<CodeFolder>) -> LaunchSpec {
        LaunchSpec {
            binary: data.join("bin").join("omp"),
            layout: OmpLayout::new(data),
            model: model(),
            system_prompt: "prompt".into(),
            session_id: "s1".into(),
            code,
        }
    }

    #[test]
    fn research_sessions_run_without_tools_in_an_empty_folder() {
        let data = tempfile::tempdir().unwrap();
        let spec = spec(data.path(), None);
        let (args, cwd) = spec.command_line().unwrap();
        assert_eq!(&args[..12], &BASE_FLAGS.map(String::from));
        assert!(!args
            .iter()
            .any(|a| a.starts_with("--tools") || a.starts_with("--hook")));
        assert_eq!(cwd, spec.layout.sessions.join("s1"));
        assert_eq!(std::fs::read_dir(&cwd).unwrap().count(), 0);
        let env = spec.environment();
        assert!(!env
            .iter()
            .any(|(k, _)| k == CODE_ROOT_ENV || k == EDIT_VARIANT_ENV.0));
    }

    #[test]
    fn code_sessions_run_with_the_code_tools_in_the_code_folder() {
        let data = tempfile::tempdir().unwrap();
        let code = tempfile::tempdir().unwrap();
        std::fs::write(code.path().join("main.rs"), "fn main() {}").unwrap();
        let folder = CodeFolder::open(code.path()).unwrap();
        let spec = spec(data.path(), Some(folder.clone()));
        let (args, cwd) = spec.command_line().unwrap();
        assert!(args.contains(&"--tools=read,grep,glob,ast_grep,edit,write,bash".to_string()));
        assert!(!args.iter().any(|a| a == "--no-tools" || a == "--no-ui"));
        assert!(args.contains(&format!("--hook={}", spec.layout.guard_hook.display())));
        assert!(args.contains(&format!("--config={}", spec.layout.code_overlay.display())));
        // The folder is worked in, never emptied.
        assert_eq!(cwd, folder.root());
        assert!(code.path().join("main.rs").exists());
        // The guard and overlay live in app data, outside the code folder.
        assert_eq!(
            std::fs::read_to_string(&spec.layout.guard_hook).unwrap(),
            GUARD_HOOK
        );
        let overlay: Value =
            serde_json::from_str(&std::fs::read_to_string(&spec.layout.code_overlay).unwrap())
                .unwrap();
        assert_eq!(overlay, code_overlay_config());
        assert!(!spec.layout.guard_hook.starts_with(folder.root()));
        let env = spec.environment();
        let get = |k: &str| {
            env.iter()
                .find(|(n, _)| n == k)
                .map(|(_, v)| v.as_str().to_string())
        };
        assert_eq!(get(CODE_ROOT_ENV), Some(folder.display()));
        assert_eq!(get(EDIT_VARIANT_ENV.0).as_deref(), Some("replace"));
        assert!(spec.describe().contains("code in"));
    }

    #[test]
    fn overlay_disables_background_features() {
        let overlay = overlay_config();
        assert_eq!(overlay["providers"]["cacheWarming"], "off");
        assert_eq!(overlay["retry"]["fallbackChains"]["judge"], json!([]));
        assert_eq!(overlay["memory"]["backend"], "off");
        assert_eq!(overlay["skills"]["enabled"], false);
    }

    #[test]
    fn sums_parsing_finds_the_asset() {
        let sums = format!(
            "{}  omp-linux-x64\n{} *omp-windows-x64.exe\n",
            "a".repeat(64),
            OMP_WINDOWS_X64_SHA256
        );
        assert_eq!(hash_from_sums(&sums, "omp-linux-x64"), Some("a".repeat(64)));
        assert_eq!(
            hash_from_sums(&sums, "omp-windows-x64.exe").as_deref(),
            Some(OMP_WINDOWS_X64_SHA256)
        );
        assert_eq!(hash_from_sums(&sums, "omp-darwin-arm64"), None);
        assert_eq!(hash_from_sums("zz  omp-linux-x64", "omp-linux-x64"), None);
    }

    #[tokio::test]
    async fn binary_verification_checks_existence_and_hash() {
        let dir = std::env::temp_dir().join(format!("shodh-omp-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let missing = dir.join("omp.exe");
        assert!(matches!(
            verify_binary(&missing).await,
            Err(HarnessError::BinaryMissing { .. })
        ));
        std::fs::write(&missing, b"not omp").unwrap();
        assert_eq!(
            sha256_file(&missing).await.unwrap(),
            hex::encode(Sha256::digest(b"not omp"))
        );
        if pinned_hash().is_some() {
            assert!(matches!(
                verify_binary(&missing).await,
                Err(HarnessError::HashMismatch { .. })
            ));
        } else {
            std::fs::write(
                recorded_hash_path(&missing),
                format!("{}\n", sha256_file(&missing).await.unwrap()),
            )
            .unwrap();
            assert!(verify_binary(&missing).await.is_ok());
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
