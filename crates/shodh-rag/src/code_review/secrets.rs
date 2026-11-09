//! Files of a code folder that Shodh never reads into a review, a diff or the
//! code pane: the repository's own `.git` folder (its config can hold tokens
//! in remote URLs), environment files, private keys and credential files, by
//! the same names Shodh keeps commands away from in the user's home folder
//! (`run_command::policy::SENSITIVE_IN_HOME`). Files git ignores are refused
//! too, by the callers that can ask git ([`crate::harness::code_mode::git_ignores`]).

/// What a file whose lines are kept back for this reason says.
pub const SECRET_NOTE: &str =
    "Not shown: this file may hold secrets (keys, tokens, passwords), so Shodh does not read it.";

/// Folder names whose files are never read.
const SECRET_FOLDERS: [&str; 8] = [
    ".git", ".ssh", ".gnupg", ".aws", ".azure", ".kube", ".docker", ".secrets",
];

/// File names that hold credentials, exactly.
const SECRET_NAMES: [&str; 19] = [
    ".netrc",
    ".pgpass",
    ".my.cnf",
    "auth.json",
    ".credentials.json",
    "rclone.conf",
    ".dockercfg",
    "kubeconfig",
    "_netrc",
    ".npmrc",
    ".pypirc",
    ".git-credentials",
    ".htpasswd",
    "id_rsa",
    "id_dsa",
    "id_ecdsa",
    "id_ed25519",
    "credentials",
    "credentials.toml",
];

/// File endings of keys and certificates with private parts.
const SECRET_ENDINGS: [&str; 11] = [
    ".tfvars",
    ".tfstate",
    ".pem",
    ".key",
    ".p12",
    ".pfx",
    ".jks",
    ".keystore",
    ".kdbx",
    ".ppk",
    ".asc",
];

/// `.env` templates, which hold names, not values.
const ENV_TEMPLATES: [&str; 4] = [".example", ".sample", ".template", ".dist"];

/// Why `path` (relative to the code folder, `/` or `\` separated) is never
/// read, or `None` when it may be.
pub fn secret_path(path: &str) -> Option<&'static str> {
    let normal = path.replace('\\', "/");
    let parts: Vec<&str> = normal
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    let (name, folders) = parts.split_last()?;
    let lower = name.to_ascii_lowercase();
    if folders
        .iter()
        .any(|f| SECRET_FOLDERS.contains(&f.to_ascii_lowercase().as_str()))
        || SECRET_FOLDERS.contains(&lower.as_str())
    {
        return Some("it is in a folder that holds credentials or the repository's own settings");
    }
    if lower == ".env" || lower.starts_with(".env.") {
        let template = ENV_TEMPLATES.iter().any(|t| lower.ends_with(t));
        if !template {
            return Some("it is an environment file, which holds keys and passwords");
        }
    }
    if lower.ends_with(".env") && !lower.starts_with('.') {
        return Some("it is an environment file, which holds keys and passwords");
    }
    if SECRET_NAMES.contains(&lower.as_str())
        || lower.starts_with("id_rsa")
        || lower.starts_with("id_ed25519")
        || lower.starts_with("id_ecdsa")
        || lower.starts_with("credentials.")
        || lower.starts_with("secrets.")
    {
        return Some("its name is one credentials are kept under");
    }
    if SECRET_ENDINGS.iter().any(|ending| lower.ends_with(ending)) {
        return Some("it is a key or certificate file");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_credentials_and_the_git_folder_are_never_read() {
        for path in [
            ".git/config",
            "sub/.git/HEAD",
            ".env",
            "app/.env.local",
            "prod.env",
            "deploy/id_rsa",
            "deploy/id_ed25519.pub",
            "certs/server.pem",
            "certs/server.KEY",
            ".npmrc",
            "config\\credentials.json",
            "secrets.yaml",
            ".ssh/known_hosts",
            "/home/u/.pgpass",
            "C:/Users/u/.codex/auth.json",
            "/home/u/.claude/.credentials.json",
            "infra/prod.tfvars",
            "infra/terraform.tfstate",
        ] {
            assert!(secret_path(path).is_some(), "{path}");
        }
        for path in [
            "src/lib.rs",
            ".env.example",
            "app/.env.template",
            "docs/environment.md",
            "src/keyboard.rs",
            "README.md",
        ] {
            assert_eq!(secret_path(path), None, "{path}");
        }
    }
}
