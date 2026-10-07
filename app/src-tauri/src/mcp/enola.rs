//! enola, the first curated MCP server: architecture maps of a code folder
//! (<https://github.com/enola-labs/enola>).
//!
//! "Add enola" downloads the pinned release for this platform, checks the
//! archive against the SHA-256 pinned here and against the release's own
//! `.sha256` file, unpacks only the binary into the app data directory and
//! registers it in the global `mcp.json`: `enola` with no arguments is its
//! stdio MCP server, run in the workspace's code folder, offered in Code
//! mode only, every tool `auto` (it reads code and writes only its own
//! `.enola/` index).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::json;
use sha2::{Digest, Sha256};
use shodh_rag::harness::mcp::config::{self, WORKSPACE_FOLDER_VAR};
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

/// The `mcp.json` entry for the installed binary.
pub fn server_entry(binary: &Path) -> serde_json::Value {
    json!({
        "command": binary.display().to_string(),
        "cwd": WORKSPACE_FOLDER_VAR,
        "shodh": { "modes": ["code"], "approval": "auto" }
    })
}

/// Add (or update) the enola entry of a config document.
pub fn register(doc: &mut serde_json::Value, binary: &Path) -> Result<(), config::ConfigError> {
    config::add_servers(doc, vec![(SERVER_NAME.to_string(), server_entry(binary))])
}

#[cfg(test)]
mod tests {
    use super::*;
    use shodh_rag::harness::mcp::{Approval, Mode};

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
    fn enola_is_registered_for_code_mode_with_every_tool_auto() {
        let mut doc = config::empty_doc();
        register(&mut doc, Path::new("C:/data/tools/enola-0.4.27/enola.exe")).unwrap();
        let server = config::parse_doc(&doc).unwrap().servers.remove(0);
        assert_eq!(server.name, "enola");
        assert!(server.in_mode(Mode::Code) && !server.in_mode(Mode::Research));
        assert_eq!(
            server.approval_for("generate_snapshot", false),
            Approval::Auto
        );
        assert!(server.transport.needs_workspace_folder());
        assert_eq!(
            server.transport.summary(),
            "C:/data/tools/enola-0.4.27/enola.exe"
        );
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
