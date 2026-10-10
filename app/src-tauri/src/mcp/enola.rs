//! enola, the first curated MCP server: architecture maps of a code folder
//! (<https://github.com/enola-labs/enola>).
//!
//! "Add enola" downloads the pinned release for this platform, checks the
//! archive against the SHA-256 pinned here and against the release's own
//! `.sha256` file, unpacks only the binary into the app data directory and
//! registers it in the global `mcp.json`: `enola --no-dashboard` is its stdio
//! MCP server, run in the workspace's code folder and offered in Code mode
//! only.
//!
//! Approval ([`TOOLS`], from reading the v0.4.27 source,
//! `internal/server/*.go` and `internal/{metrics,orphans,perf}`, and calling
//! each of the 22 tools alone on a scratch repository with the repository and
//! `~/.enola` listed before and after): `generate_snapshot` writes the
//! `.enola/` index in the code folder (and `~/.enola/receipt.json`) and runs
//! the "providers" a repository's `mcp-arch.yaml` declares, which are
//! programs; `set_baseline` writes `.enola/baseline/`. Both ask. The other 20
//! change nothing in the code folder ("read-only" here; every call updates
//! enola's own bookkeeping under `~/.enola/usage` and `~/.enola/instances`)
//! and run by themselves, except when an argument points them at another
//! folder: `repo_path` of the history tools, a directory as `baseline`.
//!
//! Network (observed with a recording proxy that refuses every connection
//! and the process's sockets sampled every 100 ms while all 22 tools were
//! called on a scratch repository): by default enola fetches its update
//! manifest from `github.com:443` when it starts and serves a dashboard on
//! `127.0.0.1:7171` and an ephemeral loopback port. With `--no-dashboard`
//! and `ENOLA_NO_UPDATE_CHECK=1`, as Shodh starts it ([`harden`]), none was
//! observed: no proxied request and no socket at all.
//!
//! `.enola/` is added to the repository's local exclude file before enola
//! starts in a folder, so Code mode's clean-tree check and "Discard changes"
//! ignore the index.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::json;
use sha2::{Digest, Sha256};
use shodh_rag::harness::mcp::config::{self, WORKSPACE_FOLDER_VAR};
use shodh_rag::harness::mcp::{ServerConfig, Transport, Verified};
use shodh_rag::harness::sidecar::hash_from_sums;

/// Pinned enola release.
pub const VERSION: &str = "0.4.27";

/// Name of the server in `mcp.json`.
pub const SERVER_NAME: &str = "enola";

const RELEASE_URL: &str = "https://github.com/enola-labs/enola/releases/download/v0.4.27";

/// SHA-256 of `enola-0.4.27-<platform>.tar.gz`, from the v0.4.27 release.
const PINNED: [(&str, &str); 5] = [
    (
        "windows-amd64",
        "7903ea4e84c4fa79058124fcb0307a183d35fe807d2a84ba8b14ba4651b791a8",
    ),
    (
        "linux-amd64",
        "196d456cf8e9f3594e21c8aa602074f877171e65125e852aa46efc9eb6bfbf06",
    ),
    (
        "linux-arm64",
        "820b88baa3ef9efd984e850004c9decc1d0b5c2d5381bb04487c332dd9360ef3",
    ),
    (
        "darwin-amd64",
        "afc917d95bcd864595e5a1283128c22c9d774eba4eb5292fe35156b52045f9c6",
    ),
    (
        "darwin-arm64",
        "59c2e30de948b5b70af9715f0a3adfb86162531e527e779b036b8483d73ae34d",
    ),
];

/// Starts the MCP server without its loopback dashboard.
pub const NO_DASHBOARD: &str = "--no-dashboard";

/// Turns off the update check (a request to github.com at start-up).
pub const NO_UPDATE_CHECK: (&str, &str) = ("ENOLA_NO_UPDATE_CHECK", "1");

/// enola's index in the code folder, kept out of git locally.
pub const INDEX_EXCLUDE: &str = ".enola/";

const BASELINES: &[&str] = &["pinned", "previous"];

/// Every tool of the pinned build and what it does to the code folder.
pub const TOOLS: [(&str, Verified); 22] = [
    ("analyze_performance", Verified::ReadOnly),
    (
        "architecture_blame",
        Verified::ReadOnlyUnless {
            arg: "repo_path",
            allowed: &[],
        },
    ),
    (
        "architecture_history",
        Verified::ReadOnlyUnless {
            arg: "repo_path",
            allowed: &[],
        },
    ),
    (
        "compare_receipts",
        Verified::ReadOnlyUnless {
            arg: "baseline",
            allowed: BASELINES,
        },
    ),
    ("constraints_for", Verified::ReadOnly),
    ("coverage_report", Verified::ReadOnly),
    (
        "diff_snapshot",
        Verified::ReadOnlyUnless {
            arg: "baseline",
            allowed: BASELINES,
        },
    ),
    ("endpoint_impact", Verified::ReadOnly),
    ("explore", Verified::ReadOnly),
    ("find_orphans", Verified::ReadOnly),
    ("find_path", Verified::ReadOnly),
    ("generate_snapshot", Verified::Writes),
    ("governing_intent", Verified::ReadOnly),
    ("impact_analysis", Verified::ReadOnly),
    ("package_metrics", Verified::ReadOnly),
    ("plan_check", Verified::ReadOnly),
    ("query_facts", Verified::ReadOnly),
    ("query_insights", Verified::ReadOnly),
    ("set_baseline", Verified::Writes),
    ("show_symbol", Verified::ReadOnly),
    ("snapshot_receipt", Verified::ReadOnly),
    ("traverse", Verified::ReadOnly),
];

/// Largest archive accepted (the releases are about 12 MB).
const MAX_ARCHIVE_BYTES: u64 = 64 * 1024 * 1024;

/// enola's name for this platform, if it publishes a build for it.
pub fn platform() -> Option<&'static str> {
    if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        Some("windows-amd64")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("linux-amd64")
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Some("linux-arm64")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Some("darwin-amd64")
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("darwin-arm64")
    } else {
        None
    }
}

/// The pinned archive hash for `platform`.
pub fn pinned_hash(platform: &str) -> Option<&'static str> {
    PINNED
        .iter()
        .find(|(p, _)| *p == platform)
        .map(|(_, hash)| *hash)
}

fn archive_name(platform: &str) -> String {
    format!("enola-{VERSION}-{platform}.tar.gz")
}

/// The binary's name inside the archive.
fn binary_in_archive(platform: &str) -> String {
    let ext = if platform.starts_with("windows") {
        ".exe"
    } else {
        ""
    };
    format!("enola-{VERSION}-{platform}{ext}")
}

/// Where the binary is installed.
pub fn binary_path(data_dir: &Path) -> PathBuf {
    let name = if cfg!(windows) { "enola.exe" } else { "enola" };
    data_dir
        .join("tools")
        .join(format!("enola-{VERSION}"))
        .join(name)
}

/// Check the release's `.sha256` text and the archive against the pinned hash.
pub fn verify(platform: &str, sums: &str, archive: &[u8]) -> Result<(), String> {
    let pinned = pinned_hash(platform).ok_or("enola has no build for this computer")?;
    let listed = hash_from_sums(sums, &archive_name(platform))
        .ok_or("the release's checksum file does not list the archive")?;
    if listed != pinned {
        return Err("the release's checksum differs from the one Shodh expects".into());
    }
    let actual = hex::encode(Sha256::digest(archive));
    if actual != pinned {
        return Err(format!(
            "the download is not the expected file (SHA-256 {actual}, expected {pinned})"
        ));
    }
    Ok(())
}

/// Write the archive's binary to `dest` (nothing else is unpacked).
pub fn extract_binary(platform: &str, archive: &[u8], dest: &Path) -> Result<(), String> {
    let wanted = binary_in_archive(platform);
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    let entries = tar.entries().map_err(|e| e.to_string())?;
    for entry in entries {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path().map_err(|e| e.to_string())?.into_owned();
        let is_binary = entry.header().entry_type().is_file()
            && path.components().count() == 1
            && path.file_name().and_then(|n| n.to_str()) == Some(wanted.as_str());
        if !is_binary {
            continue;
        }
        let mut bytes = Vec::new();
        entry
            .by_ref()
            .take(MAX_ARCHIVE_BYTES * 8)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        let dir = dest.parent().ok_or("invalid install folder")?;
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let tmp = dest.with_extension("part");
        std::fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))
                .map_err(|e| e.to_string())?;
        }
        return std::fs::rename(&tmp, dest).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            e.to_string()
        });
    }
    Err(format!("the archive has no {wanted}"))
}

async fn get(client: &reqwest::Client, url: &str) -> Result<Vec<u8>, String> {
    let mut response = client
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| format!("{url}: {e}"))?;
    if response.content_length().unwrap_or(0) > MAX_ARCHIVE_BYTES {
        return Err(format!("{url}: the file is larger than expected"));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| format!("{url}: {e}"))? {
        body.extend_from_slice(&chunk);
        if body.len() as u64 > MAX_ARCHIVE_BYTES {
            return Err(format!("{url}: the file is larger than expected"));
        }
    }
    Ok(body)
}

/// Download, verify and unpack enola; returns the binary's path.
pub async fn install(data_dir: &Path) -> Result<PathBuf, String> {
    let platform = platform().ok_or("enola has no build for this computer")?;
    let client = reqwest::Client::builder()
        .https_only(true)
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(10 * 60))
        .user_agent(concat!("shodh/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())?;
    let archive = archive_name(platform);
    let sums = get(
        &client,
        &format!("{RELEASE_URL}/enola-{VERSION}-{platform}.sha256"),
    )
    .await?;
    let bytes = get(&client, &format!("{RELEASE_URL}/{archive}")).await?;
    verify(platform, &String::from_utf8_lossy(&sums), &bytes)?;
    let dest = binary_path(data_dir);
    let target = dest.clone();
    tokio::task::spawn_blocking(move || extract_binary(platform, &bytes, &target))
        .await
        .map_err(|e| e.to_string())??;
    tracing::info!(target: "shodh::mcp", "installed enola {VERSION} at {}", dest.display());
    Ok(dest)
}

/// The `mcp.json` entry for the installed binary. Approval comes from
/// [`TOOLS`], not from the entry.
pub fn server_entry(binary: &Path) -> serde_json::Value {
    json!({
        "command": binary.display().to_string(),
        "args": [NO_DASHBOARD],
        "env": { NO_UPDATE_CHECK.0: NO_UPDATE_CHECK.1 },
        "cwd": WORKSPACE_FOLDER_VAR,
        "shodh": { "modes": ["code"] }
    })
}

/// A path compared the way the file system does (case and separators do
/// not matter on Windows).
fn path_key(path: &Path) -> String {
    let text = path.to_string_lossy();
    if cfg!(windows) {
        text.replace('/', "\\").to_lowercase()
    } else {
        text.into_owned()
    }
}

/// Whether `server` runs the binary "Add enola" installed (the build
/// [`TOOLS`] describes), whatever the server is called.
pub fn is_pinned(server: &ServerConfig, data_dir: &Path) -> bool {
    match &server.transport {
        Transport::Stdio { command, .. } => {
            path_key(Path::new(command.trim())) == path_key(&binary_path(data_dir))
        }
        Transport::Http { .. } => false,
    }
}

/// The verified classification of `server`'s tools (none unless it runs the
/// pinned build).
pub fn verified(server: &ServerConfig, data_dir: &Path) -> &'static [(&'static str, Verified)] {
    if is_pinned(server, data_dir) {
        &TOOLS
    } else {
        &[]
    }
}

/// The pinned build's launch with the dashboard and the update check off,
/// also for entries registered before Shodh set them or edited since.
pub fn harden(transport: Transport) -> Transport {
    match transport {
        Transport::Stdio {
            command,
            mut args,
            mut env,
            cwd,
        } => {
            if !args.iter().any(|a| a == NO_DASHBOARD) {
                args.insert(0, NO_DASHBOARD.to_string());
            }
            env.insert(NO_UPDATE_CHECK.0.to_string(), NO_UPDATE_CHECK.1.to_string());
            Transport::Stdio {
                command,
                args,
                env,
                cwd,
            }
        }
        other => other,
    }
}

/// Add (or update) the enola entry of a config document.
pub fn register(doc: &mut serde_json::Value, binary: &Path) -> Result<(), config::ConfigError> {
    config::add_servers(doc, vec![(SERVER_NAME.to_string(), server_entry(binary))])
}

#[cfg(test)]
mod tests {
    use super::*;
    use shodh_rag::harness::mcp::{effective_read_only, verified_for, Approval, Mode, ToolInfo};

    fn archive_with(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        {
            let mut tar = tar::Builder::new(&mut gz);
            for (name, data) in files {
                let mut header = tar::Header::new_gnu();
                header.set_size(data.len() as u64);
                header.set_mode(0o755);
                header.set_cksum();
                tar.append_data(&mut header, name, *data).unwrap();
            }
            tar.finish().unwrap();
        }
        gz.finish().unwrap()
    }

    #[test]
    fn every_supported_platform_has_a_pinned_hash() {
        for (platform, hash) in PINNED {
            assert_eq!(hash.len(), 64, "{platform}");
            assert!(hash
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        }
        if let Some(platform) = platform() {
            assert!(pinned_hash(platform).is_some());
        }
        assert_eq!(
            binary_in_archive("windows-amd64"),
            "enola-0.4.27-windows-amd64.exe"
        );
        assert_eq!(binary_in_archive("linux-arm64"), "enola-0.4.27-linux-arm64");
    }

    #[test]
    fn a_download_must_match_the_pinned_hash_and_the_release_checksum() {
        let platform = "linux-amd64";
        let pinned = pinned_hash(platform).unwrap();
        let sums = format!("{pinned}  {}\n", archive_name(platform));
        // Some other bytes than the release: refused even with a good checksum file.
        let error = verify(platform, &sums, b"not the release").unwrap_err();
        assert!(error.contains("not the expected file"), "{error}");
        // A checksum file that disagrees with the pin is refused before hashing.
        let other = format!("{}  {}\n", "0".repeat(64), archive_name(platform));
        assert!(verify(platform, &other, b"x")
            .unwrap_err()
            .contains("differs"));
        let unlisted = format!("{pinned}  enola-other.tar.gz\n");
        assert!(verify(platform, &unlisted, b"x").is_err());
        assert!(verify("plan9-mips", &sums, b"x").is_err());
    }

    #[test]
    fn only_the_binary_is_unpacked() {
        let archive = archive_with(&[
            ("LICENSE", b"license"),
            ("bin/enola-0.4.27-windows-amd64.exe", b"nested copy"),
            ("enola-0.4.27-windows-amd64.exe", b"MZ binary"),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("tools").join("enola.exe");
        extract_binary("windows-amd64", &archive, &dest).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"MZ binary");
        assert!(!dir.path().join("tools").join("LICENSE").exists());
        assert!(!dir.path().join("tools").join("bin").exists());
        let missing = archive_with(&[("LICENSE", b"license")]);
        assert!(extract_binary("windows-amd64", &missing, &dest).is_err());
    }

    #[test]
    fn enola_is_registered_for_code_mode_without_dashboard_or_update_check() {
        let data = Path::new("C:/data");
        let mut doc = config::empty_doc();
        register(&mut doc, &binary_path(data)).unwrap();
        let server = config::parse_doc(&doc).unwrap().servers.remove(0);
        assert_eq!(server.name, "enola");
        assert!(server.in_mode(Mode::Code) && !server.in_mode(Mode::Research));
        assert_eq!(server.approval, None);
        assert!(server.transport.needs_workspace_folder());
        match &server.transport {
            Transport::Stdio { args, env, .. } => {
                assert_eq!(args, &[NO_DASHBOARD.to_string()]);
                assert_eq!(env.get(NO_UPDATE_CHECK.0).map(String::as_str), Some("1"));
            }
            other => panic!("{other:?}"),
        }
        assert!(is_pinned(&server, data));
        assert_eq!(verified(&server, data).len(), 22);
        let elsewhere = Path::new("C:/other");
        assert!(!is_pinned(&server, elsewhere));
        assert!(verified(&server, elsewhere).is_empty());
    }

    #[test]
    fn only_the_tools_that_write_ask() {
        let mut names: Vec<&str> = TOOLS.iter().map(|(n, _)| *n).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), TOOLS.len());
        let writes: Vec<&str> = TOOLS
            .iter()
            .filter(|(_, v)| *v == Verified::Writes)
            .map(|(n, _)| *n)
            .collect();
        assert_eq!(writes, ["generate_snapshot", "set_baseline"]);
        let guarded: Vec<&str> = TOOLS
            .iter()
            .filter(|(_, v)| matches!(v, Verified::ReadOnlyUnless { .. }))
            .map(|(n, _)| *n)
            .collect();
        assert_eq!(
            guarded,
            [
                "architecture_blame",
                "architecture_history",
                "compare_receipts",
                "diff_snapshot"
            ]
        );

        // An entry registered by an older build (server-wide auto, no flags).
        let data = Path::new("C:/data");
        let entry = json!({
            "command": binary_path(data).display().to_string(),
            "cwd": WORKSPACE_FOLDER_VAR,
            "shodh": { "modes": ["code"], "approval": "auto" }
        });
        let server = config::parse_server("enola", &entry).unwrap();
        let info = |name: &str| -> ToolInfo {
            serde_json::from_value(json!({"name": name, "inputSchema": {"type": "object"}}))
                .unwrap()
        };
        let approval = |name: &str| {
            let checked = verified_for(verified(&server, data), name);
            let read_only = effective_read_only(&info(name), checked);
            server.approval_for(name, read_only, Mode::Code)
        };
        assert_eq!(approval("generate_snapshot"), Approval::Ask);
        assert_eq!(approval("set_baseline"), Approval::Ask);
        assert_eq!(approval("explore"), Approval::Auto);
        assert_eq!(approval("diff_snapshot"), Approval::Auto);
        match harden(server.transport.clone()) {
            Transport::Stdio { args, env, .. } => {
                assert_eq!(args, [NO_DASHBOARD]);
                assert_eq!(env.get(NO_UPDATE_CHECK.0).map(String::as_str), Some("1"));
            }
            other => panic!("{other:?}"),
        }
        let hardened = harden(harden(server.transport.clone()));
        assert!(matches!(hardened, Transport::Stdio { ref args, .. } if args.len() == 1));
    }

    /// Against GitHub: the pinned release downloads, verifies and serves its
    /// tools over stdio in a code folder.
    #[tokio::test]
    #[ignore = "downloads enola from github.com"]
    async fn the_pinned_release_installs_and_lists_its_tools() {
        let data = tempfile::tempdir().unwrap();
        let binary = install(data.path()).await.unwrap();
        let repo = tempfile::tempdir().unwrap();
        std::fs::write(
            repo.path().join("main.go"),
            "package main\n\nfunc main() {}\n",
        )
        .unwrap();
        let mut doc = config::empty_doc();
        register(&mut doc, &binary).unwrap();
        let server = config::parse_doc(&doc).unwrap().servers.remove(0);
        let transport = server
            .transport
            .resolved(Some(&repo.path().display().to_string()))
            .unwrap();
        let client = shodh_rag::harness::mcp::McpClient::connect(&transport)
            .await
            .unwrap();
        let tools = client.list_tools().await.unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(
            names.contains(&"explore") && names.contains(&"generate_snapshot"),
            "{names:?}"
        );
        client.shutdown();
    }
}
