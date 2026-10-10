//! Corpus folders: document keys, content hashes and where run outputs go.
//!
//! A document key is a file's path relative to the corpus folder, spelled the
//! way the engine stores sources ([`normalize_source_path`]: forward slashes,
//! lower-case on Windows). Expected files and retrieved sources are both
//! mapped through it, so they compare equal however the path was typed.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use shodh_rag::indexing::is_supported_file_type;
use shodh_rag::rag_engine::{canonical_path, normalize_source_path};

/// Directory (relative to the working directory) for runs, generated
/// questions and models. Ignored by git; never commit its contents.
pub const LOCAL_DIR: &str = ".eval-local";

/// The engine's spelling of the corpus root.
pub fn root_key(root: &Path) -> String {
    normalize_source_path(root)
}

/// The document key of an indexed `source` under the corpus root, or `None`
/// when the source is not a file inside it.
pub fn relative_key(root_key: &str, source: &str) -> Option<String> {
    if source.contains("://") {
        return None;
    }
    let source = normalize_source_path(Path::new(source));
    let rest = source.strip_prefix(root_key)?.strip_prefix('/')?;
    (!rest.is_empty()).then(|| rest.to_string())
}

/// The document key of a dataset's corpus-relative `file`.
pub fn expected_key(root: &Path, file: &str) -> String {
    let joined = normalize_source_path(&root.join(file.replace('\\', "/")));
    relative_key(&root_key(root), &joined).unwrap_or(joined)
}

/// The folder as an absolute path without a verbatim prefix (the spelling
/// the indexer stores).
pub fn absolute_dir(path: &Path) -> Result<PathBuf> {
    let full = std::fs::canonicalize(path)
        .with_context(|| format!("corpus folder {} not found", path.display()))?;
    if !full.is_dir() {
        anyhow::bail!("{} is not a folder", path.display());
    }
    Ok(canonical_path(&full))
}

/// Every file under `root` as (corpus-relative path with `/`, absolute path),
/// sorted by relative path.
fn files(root: &Path) -> Result<Vec<(String, PathBuf)>> {
    let mut out = Vec::new();
    for entry in walkdir::WalkDir::new(root).sort_by_file_name() {
        let entry = entry.with_context(|| format!("listing {}", root.display()))?;
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(root)
            .with_context(|| format!("{} is outside the corpus", entry.path().display()))?;
        let rel = rel.to_string_lossy().replace('\\', "/");
        out.push((rel, entry.path().to_path_buf()));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// Corpus-relative paths of the files the indexer accepts.
pub fn supported_files(root: &Path) -> Result<Vec<String>> {
    Ok(files(root)?
        .into_iter()
        .map(|(rel, _)| rel)
        .filter(|rel| {
            Path::new(rel)
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| is_supported_file_type(&e.to_lowercase()))
        })
        .collect())
}

/// SHA-256 over every file's relative path and bytes: a report is only
/// comparable with a baseline measured on the same corpus.
pub fn corpus_hash(root: &Path) -> Result<String> {
    let mut hasher = Sha256::new();
    for (rel, path) in files(root)? {
        let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        hasher.update(rel.as_bytes());
        hasher.update([0u8]);
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(&bytes);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Default output folder for a corpus: `.eval-local/runs/<name>-<hash>`,
/// where the hash is of the corpus path (two folders with one name differ).
pub fn default_output_dir(corpus: &Path) -> PathBuf {
    let name: String = corpus
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "corpus".to_string())
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let digest = hex::encode(Sha256::digest(root_key(corpus).as_bytes()));
    Path::new(LOCAL_DIR)
        .join("runs")
        .join(format!("{name}-{}", &digest[..8]))
}

/// Create `dir` and mark it ignored by git (a `.gitignore` of `*`), so run
/// outputs (questions and answers about private documents) cannot be
/// committed by accident wherever they are written.
pub fn prepare_output_dir(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let ignore = dir.join(".gitignore");
    if !ignore.exists() {
        std::fs::write(&ignore, "*\n").with_context(|| format!("writing {}", ignore.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sources_map_back_to_keys() {
        let dir = tempfile::tempdir().unwrap();
        let root = absolute_dir(dir.path()).unwrap();
        let key = root_key(&root);
        let source = normalize_source_path(&root.join("Invoices").join("INV-1.pdf"));
        let expected = if cfg!(windows) {
            "invoices/inv-1.pdf"
        } else {
            "Invoices/INV-1.pdf"
        };
        assert_eq!(relative_key(&key, &source).as_deref(), Some(expected));
        assert_eq!(expected_key(&root, "Invoices\\INV-1.pdf"), expected);
        assert_eq!(expected_key(&root, "Invoices/INV-1.pdf"), expected);
    }

    #[test]
    fn sources_outside_the_root_are_unmapped() {
        let dir = tempfile::tempdir().unwrap();
        let root = absolute_dir(dir.path()).unwrap();
        let key = root_key(&root);
        let sibling = format!("{key}-2/x.pdf");
        assert_eq!(relative_key(&key, &sibling), None);
        assert_eq!(relative_key(&key, "note://abc"), None);
        assert_eq!(relative_key(&key, &key), None);
    }

    #[test]
    fn hash_covers_names_and_bytes_and_skips_nothing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub").join("a.txt"), b"one").unwrap();
        std::fs::write(dir.path().join("b.bin"), b"two").unwrap();
        let first = corpus_hash(dir.path()).unwrap();
        assert_eq!(first, corpus_hash(dir.path()).unwrap());
        std::fs::write(dir.path().join("b.bin"), b"twO").unwrap();
        assert_ne!(first, corpus_hash(dir.path()).unwrap());
        assert_eq!(supported_files(dir.path()).unwrap(), vec!["sub/a.txt"]);
    }

    #[test]
    fn output_dirs_are_git_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("runs").join("x");
        prepare_output_dir(&out).unwrap();
        assert_eq!(
            std::fs::read_to_string(out.join(".gitignore")).unwrap(),
            "*\n"
        );
        let named = default_output_dir(Path::new("C:/Users/me/My Papers"));
        assert!(named.starts_with(LOCAL_DIR));
        assert!(named.to_string_lossy().contains("My_Papers-"));
    }
}
