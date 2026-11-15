//! The phone's one-tap update. The phone and the computer must run the same
//! Shodh version; when the computer is newer, the phone fetches the matching
//! Android app from that version's GitHub release and hands it to Android's
//! installer only once it is verified:
//!
//! - its minisign signature, by Shodh's release key (the key the desktop's
//!   updater trusts, compiled into the phone app), over the whole file;
//! - when the release lists one, its SHA-256 (GitHub's `digest` of the
//!   asset) and its size.
//!
//! Android then checks the app is signed by the same key as the installed
//! one. A phone newer than its computer is never "updated" backwards: it
//! asks for the computer to be updated instead.

use std::io::Read;

use data_encoding::{BASE64, HEXLOWER_PERMISSIVE};
use ring::digest::{Context, SHA256};
use serde::Serialize;
use serde_json::Value;

/// The GitHub repository Shodh is released from.
pub const REPO: &str = "varun29ankuS/shodhRAG";

/// The largest app file the phone downloads.
pub const MAX_APK_BYTES: u64 = 300 * 1024 * 1024;

/// Why an update cannot go ahead, in plain words.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UpdateError {
    #[error("“{0}” is not a Shodh version.")]
    NotAVersion(String),
    #[error("Your phone runs a newer Shodh ({phone}) than your computer ({desktop}). Update Shodh on your computer.")]
    UpdateComputer { phone: String, desktop: String },
    #[error("Shodh {0} for Android is not published yet. Try again later, or install it from the release page.")]
    NotPublished(String),
    #[error("The release of Shodh {0} is not complete: {1}")]
    Incomplete(String, String),
    #[error("The downloaded app is not the one Shodh released: {0}. It was not installed.")]
    NotGenuine(String),
    #[error("The download failed: {0}")]
    Download(String),
}

/// A Shodh version, `major.minor.patch`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u64, pub u64, pub u64);

impl Version {
    pub fn parse(text: &str) -> Result<Self, UpdateError> {
        let not = || UpdateError::NotAVersion(text.chars().take(40).collect());
        let mut parts = text.trim().split('.');
        let mut part = || -> Result<u64, UpdateError> {
            let p = parts.next().ok_or_else(not)?;
            if p.is_empty() || p.len() > 9 || !p.bytes().all(|b| b.is_ascii_digit()) {
                return Err(not());
            }
            p.parse().map_err(|_| not())
        };
        let version = Version(part()?, part()?, part()?);
        if parts.next().is_some() {
            return Err(not());
        }
        Ok(version)
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// What a version mismatch asks for.
pub fn needed(phone: &str, desktop: &str) -> Result<Version, UpdateError> {
    let ours = Version::parse(phone)?;
    let theirs = Version::parse(desktop)?;
    if ours > theirs {
        return Err(UpdateError::UpdateComputer {
            phone: ours.to_string(),
            desktop: theirs.to_string(),
        });
    }
    Ok(theirs)
}

/// The GitHub API address of version `version`'s release.
pub fn release_api_url(version: Version) -> String {
    format!("https://api.github.com/repos/{REPO}/releases/tags/v{version}")
}

/// The Android app's file name in a release (the release's contract).
pub fn apk_name(version: Version) -> String {
    format!("Shodh_{version}_android.apk")
}

/// The Android app of a release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApkRelease {
    pub version: String,
    pub apk_url: String,
    pub signature_url: String,
    pub size: u64,
    /// GitHub's SHA-256 of the file, hex, when the release lists it.
    pub sha256: Option<String>,
}

/// The app and its signature in `release` (the GitHub API's release JSON),
/// which must be version `version`'s published release, downloaded from
/// Shodh's own repository.
pub fn choose_apk(release: &Value, version: Version) -> Result<ApkRelease, UpdateError> {
    let tag = format!("v{version}");
    let incomplete = |why: &str| UpdateError::Incomplete(version.to_string(), why.to_string());
    if release.get("tag_name").and_then(Value::as_str) != Some(tag.as_str()) {
        return Err(incomplete("it is another version's release"));
    }
    if release.get("draft").and_then(Value::as_bool) == Some(true) {
        return Err(UpdateError::NotPublished(version.to_string()));
    }
    let assets = release
        .get("assets")
        .and_then(Value::as_array)
        .ok_or_else(|| UpdateError::NotPublished(version.to_string()))?;
    let name = apk_name(version);
    let signature = format!("{name}.sig");
    let find = |wanted: &str| {
        assets
            .iter()
            .find(|a| a.get("name").and_then(Value::as_str) == Some(wanted))
    };
    let apk = find(&name).ok_or_else(|| UpdateError::NotPublished(version.to_string()))?;
    let sig = find(&signature).ok_or_else(|| incomplete("the app's signature is missing"))?;
    let prefix = format!("https://github.com/{REPO}/releases/download/{tag}/");
    let url = |asset: &Value| -> Result<String, UpdateError> {
        let url = asset
            .get("browser_download_url")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if url.starts_with(&prefix) && !url[prefix.len()..].contains(['/', '?', '#']) {
            Ok(url.to_string())
        } else {
            Err(incomplete("a download is not from Shodh's release"))
        }
    };
    let size = apk.get("size").and_then(Value::as_u64).unwrap_or(0);
    if size == 0 || size > MAX_APK_BYTES {
        return Err(incomplete("the app's size is not plausible"));
    }
    let sha256 = match apk.get("digest").and_then(Value::as_str) {
        None => None,
        Some(digest) => {
            let hex = digest
                .strip_prefix("sha256:")
                .filter(|h| h.len() == 64 && HEXLOWER_PERMISSIVE.decode(h.as_bytes()).is_ok())
                .ok_or_else(|| incomplete("the app's checksum is not readable"))?;
            Some(hex.to_ascii_lowercase())
        }
    };
    Ok(ApkRelease {
        version: version.to_string(),
        apk_url: url(apk)?,
        signature_url: url(sig)?,
        size,
        sha256,
    })
}

/// The minisign public key of Shodh's releases, as the desktop's updater
/// keeps it (`plugins.updater.pubkey`: the base64 of the key file's text).
pub fn release_key(updater_pubkey: &str) -> Result<minisign_verify::PublicKey, String> {
    let text = BASE64
        .decode(updater_pubkey.trim().as_bytes())
        .map_err(|_| "The release key is not readable.".to_string())?;
    let text =
        String::from_utf8(text).map_err(|_| "The release key is not readable.".to_string())?;
    minisign_verify::PublicKey::decode(&text)
        .map_err(|e| format!("The release key is not readable: {e}"))
}

/// A signature file as releases publish it: minisign's text, or (as Tauri's
/// updater writes `.sig` files) the base64 of that text.
fn signature(text: &str) -> Result<minisign_verify::Signature, UpdateError> {
    let not = |e: String| UpdateError::NotGenuine(format!("its signature is not readable ({e})"));
    match minisign_verify::Signature::decode(text) {
        Ok(signature) => Ok(signature),
        Err(_) => {
            let decoded = BASE64
                .decode(text.trim().as_bytes())
                .map_err(|e| not(e.to_string()))?;
            let decoded = String::from_utf8(decoded).map_err(|e| not(e.to_string()))?;
            minisign_verify::Signature::decode(&decoded).map_err(|e| not(e.to_string()))
        }
    }
}

/// Check a downloaded app before it is installed: the release key's
/// signature over every byte, the size and, when the release lists it, the
/// SHA-256. Only a prehashed (current minisign) signature is accepted.
pub fn verify_apk(
    mut apk: impl Read,
    signature_text: &str,
    key: &minisign_verify::PublicKey,
    release: &ApkRelease,
) -> Result<(), UpdateError> {
    let signature = signature(signature_text)?;
    let mut verifier = key.verify_stream(&signature).map_err(|e| {
        UpdateError::NotGenuine(format!("it is not signed with Shodh's release key ({e})"))
    })?;
    let mut sha = Context::new(&SHA256);
    let mut size: u64 = 0;
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = apk
            .read(&mut buffer)
            .map_err(|e| UpdateError::Download(e.to_string()))?;
        if read == 0 {
            break;
        }
        size += read as u64;
        if size > MAX_APK_BYTES {
            return Err(UpdateError::NotGenuine(
                "it is larger than any Shodh app".into(),
            ));
        }
        verifier.update(&buffer[..read]);
        sha.update(&buffer[..read]);
    }
    if size != release.size {
        return Err(UpdateError::NotGenuine(format!(
            "it is {size} bytes, and the release says {}",
            release.size
        )));
    }
    if let Some(expected) = &release.sha256 {
        let got = HEXLOWER_PERMISSIVE.encode(sha.finish().as_ref());
        if &got != expected {
            return Err(UpdateError::NotGenuine(
                "its checksum differs from the release's".into(),
            ));
        }
    }
    verifier.finalize().map_err(|_| {
        UpdateError::NotGenuine("its signature does not match Shodh's release key".into())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use blake2::Digest;
    use iroh::SecretKey;
    use serde_json::json;

    const KEY_ID: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];

    /// A release key and its updater form (base64 of the key file's text).
    fn release_keypair() -> (SecretKey, String) {
        let secret = SecretKey::generate();
        let mut bin = b"Ed".to_vec();
        bin.extend_from_slice(&KEY_ID);
        bin.extend_from_slice(secret.public().as_bytes());
        let file = format!(
            "untrusted comment: minisign public key: 0807060504030201\n{}\n",
            BASE64.encode(&bin)
        );
        (secret, BASE64.encode(file.as_bytes()))
    }

    /// A prehashed minisign signature of `data`, as `tauri signer sign` makes.
    fn sign(secret: &SecretKey, data: &[u8]) -> String {
        let hash = blake2::Blake2b512::digest(data);
        let signature = secret.sign(&hash).to_bytes();
        let mut bin = b"ED".to_vec();
        bin.extend_from_slice(&KEY_ID);
        bin.extend_from_slice(&signature);
        let trusted = "timestamp:1763000000\tfile:Shodh_0.0.11_android.apk";
        let mut global = signature.to_vec();
        global.extend_from_slice(trusted.as_bytes());
        let global = secret.sign(&global).to_bytes();
        format!(
            "untrusted comment: signature from tauri secret key\n{}\ntrusted comment: {trusted}\n{}\n",
            BASE64.encode(&bin),
            BASE64.encode(&global)
        )
    }

    fn sha_hex(data: &[u8]) -> String {
        HEXLOWER_PERMISSIVE.encode(ring::digest::digest(&SHA256, data).as_ref())
    }

    fn release_json(version: &str, apk: &[u8], digest: Option<String>) -> Value {
        let base = format!("https://github.com/{REPO}/releases/download/v{version}");
        let mut apk_asset = json!({
            "name": format!("Shodh_{version}_android.apk"),
            "browser_download_url": format!("{base}/Shodh_{version}_android.apk"),
            "size": apk.len(),
        });
        if let Some(digest) = digest {
            apk_asset["digest"] = json!(digest);
        }
        json!({
            "tag_name": format!("v{version}"),
            "draft": false,
            "assets": [
                { "name": "Shodh_0.0.11_x64-setup.exe", "browser_download_url": format!("{base}/Shodh_0.0.11_x64-setup.exe"), "size": 9 },
                apk_asset,
                {
                    "name": format!("Shodh_{version}_android.apk.sig"),
                    "browser_download_url": format!("{base}/Shodh_{version}_android.apk.sig"),
                    "size": 400
                }
            ]
        })
    }

    #[test]
    fn a_mismatch_asks_the_older_side_to_update() {
        assert_eq!(needed("0.0.10", "0.0.11"), Ok(Version(0, 0, 11)));
        assert_eq!(
            needed("0.1.0", "0.0.11"),
            Err(UpdateError::UpdateComputer {
                phone: "0.1.0".into(),
                desktop: "0.0.11".into()
            })
        );
        for bad in [
            "",
            "1.2",
            "1.2.3.4",
            "1.2.x",
            "../1.2.3",
            "v1.2.3",
            "1.2.3-beta",
        ] {
            assert!(Version::parse(bad).is_err(), "{bad}");
        }
        assert_eq!(
            release_api_url(Version(0, 0, 11)),
            "https://api.github.com/repos/varun29ankuS/shodhRAG/releases/tags/v0.0.11"
        );
    }

    #[test]
    fn the_matching_apk_is_downloaded_and_verified_before_install() {
        let (secret, pubkey) = release_keypair();
        let key = release_key(&pubkey).unwrap();
        let apk = b"PK\x03\x04 a Shodh app".repeat(5_000);
        let release = choose_apk(
            &release_json("0.0.11", &apk, Some(format!("sha256:{}", sha_hex(&apk)))),
            Version(0, 0, 11),
        )
        .unwrap();
        assert_eq!(
            release.apk_url,
            "https://github.com/varun29ankuS/shodhRAG/releases/download/v0.0.11/Shodh_0.0.11_android.apk"
        );
        let sig = sign(&secret, &apk);
        assert_eq!(verify_apk(apk.as_slice(), &sig, &key, &release), Ok(()));
        // Tauri's updater writes the signature file as base64 of its text.
        let wrapped = BASE64.encode(sig.as_bytes());
        assert_eq!(verify_apk(apk.as_slice(), &wrapped, &key, &release), Ok(()));
    }

    #[test]
    fn an_apk_whose_signature_or_hash_does_not_match_is_refused() {
        let (secret, pubkey) = release_keypair();
        let key = release_key(&pubkey).unwrap();
        let apk = b"a Shodh app".repeat(100);
        let mut tampered = apk.clone();
        tampered[5] ^= 1;
        let sig = sign(&secret, &apk);

        // Changed bytes: the checksum the release lists differs.
        let release = choose_apk(
            &release_json("0.0.11", &apk, Some(format!("sha256:{}", sha_hex(&apk)))),
            Version(0, 0, 11),
        )
        .unwrap();
        assert!(matches!(
            verify_apk(tampered.as_slice(), &sig, &key, &release),
            Err(UpdateError::NotGenuine(_))
        ));
        // Without a listed checksum, the signature alone refuses them.
        let unlisted = choose_apk(&release_json("0.0.11", &apk, None), Version(0, 0, 11)).unwrap();
        assert!(matches!(
            verify_apk(tampered.as_slice(), &sig, &key, &unlisted),
            Err(UpdateError::NotGenuine(_))
        ));
        // Signed by another key.
        let (stranger, _) = release_keypair();
        assert!(matches!(
            verify_apk(apk.as_slice(), &sign(&stranger, &apk), &key, &unlisted),
            Err(UpdateError::NotGenuine(_))
        ));
        // A short download.
        assert!(matches!(
            verify_apk(&apk[..apk.len() - 1], &sig, &key, &unlisted),
            Err(UpdateError::NotGenuine(_))
        ));
        // A legacy (not prehashed) signature is refused.
        let legacy = sig.replacen(&BASE64.encode(b"ED")[..2], &BASE64.encode(b"Ed")[..2], 1);
        assert!(verify_apk(apk.as_slice(), &legacy, &key, &unlisted).is_err());
    }

    #[test]
    fn only_shodh_s_own_published_release_is_used() {
        let apk = b"app".to_vec();
        let mut other_repo = release_json("0.0.11", &apk, None);
        other_repo["assets"][1]["browser_download_url"] = json!(
            "https://github.com/someone/fork/releases/download/v0.0.11/Shodh_0.0.11_android.apk"
        );
        assert!(matches!(
            choose_apk(&other_repo, Version(0, 0, 11)),
            Err(UpdateError::Incomplete(..))
        ));
        let mut draft = release_json("0.0.11", &apk, None);
        draft["draft"] = json!(true);
        assert_eq!(
            choose_apk(&draft, Version(0, 0, 11)),
            Err(UpdateError::NotPublished("0.0.11".into()))
        );
        assert!(matches!(
            choose_apk(&release_json("0.0.12", &apk, None), Version(0, 0, 11)),
            Err(UpdateError::Incomplete(..))
        ));
        let mut no_apk = release_json("0.0.11", &apk, None);
        no_apk["assets"].as_array_mut().unwrap().remove(1);
        assert_eq!(
            choose_apk(&no_apk, Version(0, 0, 11)),
            Err(UpdateError::NotPublished("0.0.11".into()))
        );
        let mut bad_digest = release_json("0.0.11", &apk, Some("md5:abc".into()));
        assert!(choose_apk(&bad_digest, Version(0, 0, 11)).is_err());
        bad_digest["assets"][1]["digest"] = json!(format!("sha256:{}", "A".repeat(64)));
        assert_eq!(
            choose_apk(&bad_digest, Version(0, 0, 11)).unwrap().sha256,
            Some("a".repeat(64))
        );
    }
}
