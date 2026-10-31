//! The command sandbox's policy: what one command may touch, as one model
//! the system's boundary is rendered from (on macOS, the Seatbelt profile of
//! [`super::seatbelt`]), so what the boundary enforces and what Shodh says it
//! enforces cannot drift apart. Windows has no boundary yet: there the
//! approval prompt says commands are not sandboxed.
//!
//! - **Writable**: the code folder and the conversation's scratch folder. A
//!   command can change nothing else.
//! - **Readable**: those two, the toolchain folders commands need although
//!   `HOME` is the scratch folder (`CARGO_HOME`, `RUSTUP_HOME`), the folders
//!   on `PATH` and the shell's own install folder when they are inside the
//!   user's home, and everything outside the home that the system lets any
//!   account read (its programs and libraries).
//! - **Private**: the user's home. Nothing in it is readable unless it is one
//!   of the readable folders above.
//! - **Never readable or changed**: the sensitive folders of the home (keys,
//!   cloud and registry credentials, the keychains), where they point when
//!   they are links, the system keychains, and Shodh's own data folder, even
//!   inside a readable folder and even inside the code folder (a project
//!   folder such as `~/.config` keeps `~/.config/gh` closed). The one
//!   exception is a writable folder inside one of them: the scratch folder
//!   lives in Shodh's data folder and stays writable.
//! - **No network**, in either direction, loopback included.
//!
//! A code folder the policy cannot bound is refused, never widened: a drive
//! root, the home folder or a folder that contains it, a folder that contains
//! Shodh's data folder or lies inside it, and a folder inside a sensitive one.

use std::path::{Component, Path, PathBuf};

/// Folders of the user's home that commands never read, relative to it.
pub const SENSITIVE_IN_HOME: &[&str] = &[
    ".ssh",
    ".gnupg",
    ".aws",
    ".azure",
    ".kube",
    ".docker",
    ".config/gcloud",
    ".config/gh",
    ".config/git/credentials",
    ".git-credentials",
    ".netrc",
    "_netrc",
    ".npmrc",
    ".pypirc",
    ".cargo/credentials",
    ".cargo/credentials.toml",
    // Windows: Credential Manager, DPAPI master keys, the vault, GitHub CLI.
    "AppData/Roaming/Microsoft/Credentials",
    "AppData/Local/Microsoft/Credentials",
    "AppData/Roaming/Microsoft/Protect",
    "AppData/Roaming/Microsoft/Vault",
    "AppData/Local/Microsoft/Vault",
    "AppData/Roaming/GitHub CLI",
    // macOS: the user's keychains and cookies.
    "Library/Keychains",
    "Library/Cookies",
];

/// The system's keychains (macOS), never readable.
#[cfg(target_os = "macos")]
pub const SYSTEM_KEYCHAINS: &str = "/Library/Keychains";

/// Toolchain folders a command reads although `HOME` is the scratch folder
/// (the same names as [`super::TOOLCHAIN_HOMES`]).
const TOOLCHAIN_VARS: [&str; 2] = ["CARGO_HOME", "RUSTUP_HOME"];

/// What one command may touch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxPolicy {
    /// Folders the command may change (and read): the code folder first,
    /// then the scratch folder.
    pub writable: Vec<PathBuf>,
    /// Folders inside the private folder it may read.
    pub readable: Vec<PathBuf>,
    /// The user's home: readable only through `readable` and `writable`.
    pub private: Option<PathBuf>,
    /// Never readable or changed, even inside `readable` or `writable`; only
    /// a writable folder that lies inside one of them is open (the scratch
    /// folder in Shodh's data folder, which is among them).
    pub deny_read: Vec<PathBuf>,
    /// Network access. Always `false`: commands have no network.
    pub network: bool,
}

/// What [`SandboxPolicy::for_command`] builds a policy from.
#[derive(Debug, Clone, Copy)]
pub struct PolicyInputs<'a> {
    /// The code folder.
    pub folder: &'a Path,
    /// The conversation's scratch folder (already created).
    pub scratch: &'a Path,
    /// Shodh's data folder.
    pub data_dir: &'a Path,
    /// The user's real home.
    pub home: Option<&'a Path>,
    /// The command's environment ([`super::command_env`]).
    pub env: &'a [(String, String)],
    /// Folders of the programs the command needs (the shell's install
    /// folder), readable when they are inside the home.
    pub tools: &'a [PathBuf],
}

/// `path` with the `\\?\` prefix Windows' canonical paths carry removed when
/// it is a plain drive path, so it reads as the user wrote it.
fn plain(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\") {
        let bytes = rest.as_bytes();
        if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
            return PathBuf::from(rest.to_string());
        }
    }
    path
}

/// `path` resolved (links followed, `..` removed) when it exists, else as
/// given.
pub fn resolved(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).map_or_else(|_| path.to_path_buf(), plain)
}

/// The parts of `path` compared for containment: case-folded on Windows,
/// whose file system ignores case.
fn parts(path: &Path) -> Vec<String> {
    path.components()
        .filter(|c| !matches!(c, Component::CurDir))
        .map(|c| {
            let text = c.as_os_str().to_string_lossy().replace('\\', "/");
            if cfg!(windows) {
                text.to_lowercase()
            } else {
                text
            }
        })
        .filter(|text| text != "/" || !cfg!(windows))
        .collect()
}

/// Whether `inner` is `outer` or inside it.
pub fn within(inner: &Path, outer: &Path) -> bool {
    let inner = parts(inner);
    let outer = parts(outer);
    !outer.is_empty() && inner.len() >= outer.len() && inner[..outer.len()] == outer[..]
}

/// Whether `path` is a drive or file-system root.
fn is_root(path: &Path) -> bool {
    path.parent().is_none()
}

impl SandboxPolicy {
    /// The policy of one command, or why its folders cannot be bounded.
    pub fn for_command(inputs: PolicyInputs<'_>) -> Result<Self, String> {
        if !inputs.folder.is_dir() {
            return Err(format!(
                "the code folder {} was not found",
                inputs.folder.display()
            ));
        }
        let folder = resolved(inputs.folder);
        let scratch = resolved(inputs.scratch);
        let data_dir = resolved(inputs.data_dir);
        let home = inputs.home.map(resolved);
        let deny_read = deny_list(home.as_deref(), &data_dir, inputs.env);

        let bounded = |what: &str, path: &Path| -> Result<(), String> {
            if is_root(path) {
                return Err(format!(
                    "the {what} {} is a whole drive, so Shodh cannot keep the command to it",
                    path.display()
                ));
            }
            if let Some(home) = &home {
                if within(home, path) {
                    return Err(format!(
                        "the {what} {} is your home folder or contains it, so Shodh cannot keep \
                         the command away from your other files; open a project folder instead",
                        path.display()
                    ));
                }
            }
            if within(&data_dir, path) {
                return Err(format!(
                    "the {what} {} contains Shodh's own data, so Shodh cannot keep the command \
                     away from it",
                    path.display()
                ));
            }
            Ok(())
        };
        bounded("code folder", &folder)?;
        bounded("scratch folder", &scratch)?;
        if within(&folder, &data_dir) {
            return Err(format!(
                "the code folder {} is inside Shodh's own data, which commands never touch",
                folder.display()
            ));
        }
        if let Some(sensitive) = deny_read.iter().find(|deny| within(&folder, deny)) {
            return Err(format!(
                "the code folder {} is inside {}, which holds credentials commands never read",
                folder.display(),
                sensitive.display()
            ));
        }

        let mut readable: Vec<PathBuf> = Vec::new();
        if let Some(home) = &home {
            let path_dirs = inputs
                .env
                .iter()
                .filter(|(name, _)| name.eq_ignore_ascii_case("PATH"))
                .flat_map(|(_, value)| std::env::split_paths(value).collect::<Vec<_>>());
            let toolchains = inputs
                .env
                .iter()
                .filter(|(name, _)| TOOLCHAIN_VARS.contains(&name.as_str()))
                .map(|(_, value)| PathBuf::from(value));
            let candidates = toolchains
                .chain(inputs.tools.iter().cloned())
                .chain(path_dirs);
            for candidate in candidates {
                if !candidate.is_absolute() || !candidate.is_dir() {
                    continue;
                }
                let dir = resolved(&candidate);
                let usable = within(&dir, home)
                    && !is_root(&dir)
                    && !within(home, &dir)
                    && !within(&data_dir, &dir)
                    && !within(&dir, &data_dir)
                    && !deny_read.iter().any(|deny| within(&dir, deny))
                    && !within(&dir, &folder)
                    && !within(&dir, &scratch);
                if usable && !readable.iter().any(|known| within(&dir, known)) {
                    readable.retain(|known| !within(known, &dir));
                    readable.push(dir);
                }
            }
        }

        Ok(SandboxPolicy {
            writable: vec![folder, scratch],
            readable,
            private: home,
            deny_read,
            network: false,
        })
    }

    /// The never-readable folders that lie inside a writable folder (a
    /// credentials folder inside the code folder): closed again after the
    /// writable folders are opened.
    pub fn deny_inside_writable(&self) -> Vec<&PathBuf> {
        self.deny_read
            .iter()
            .filter(|deny| self.writable.iter().any(|root| within(deny, root)))
            .collect()
    }

    /// Whether `path` is closed by the deny list: inside one of its folders,
    /// and not inside a writable folder that itself lies in that one.
    #[cfg(test)]
    fn denied(&self, path: &Path) -> bool {
        self.deny_read.iter().any(|deny| {
            within(path, deny)
                && !self
                    .writable
                    .iter()
                    .any(|root| within(root, deny) && within(path, root))
        })
    }

    /// Whether the command may read `path` under this policy (the model the
    /// rendering implements; tests check it against it).
    #[cfg(test)]
    pub fn may_read(&self, path: &Path) -> bool {
        if self.denied(path) {
            return false;
        }
        if self.writable.iter().any(|root| within(path, root)) {
            return true;
        }
        match &self.private {
            Some(home) if within(path, home) => self.readable.iter().any(|root| within(path, root)),
            _ => true,
        }
    }

    /// Whether the command may change `path` under this policy.
    #[cfg(test)]
    pub fn may_write(&self, path: &Path) -> bool {
        !self.denied(path) && self.writable.iter().any(|root| within(path, root))
    }
}

/// Add `path` to `deny`, and the folder or file it points to when it is (or
/// lies under) a link: the system checks the real path, so a credentials
/// folder linked from elsewhere stays closed.
fn push_deny(deny: &mut Vec<PathBuf>, path: PathBuf) {
    let target = std::fs::canonicalize(&path).ok().map(plain);
    if !deny.contains(&path) {
        deny.push(path.clone());
    }
    if let Some(target) = target {
        if target != path && !deny.contains(&target) {
            deny.push(target);
        }
    }
}

/// The never-readable folders: [`SENSITIVE_IN_HOME`] in the home, the
/// credentials of a `CARGO_HOME` set elsewhere, and Shodh's data folder.
fn deny_list(home: Option<&Path>, data_dir: &Path, env: &[(String, String)]) -> Vec<PathBuf> {
    let mut deny: Vec<PathBuf> = Vec::new();
    if let Some(home) = home {
        for relative in SENSITIVE_IN_HOME {
            push_deny(
                &mut deny,
                home.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR)),
            );
        }
    }
    if let Some((_, cargo)) = env.iter().find(|(name, _)| name == "CARGO_HOME") {
        let cargo = resolved(Path::new(cargo));
        for name in ["credentials", "credentials.toml"] {
            push_deny(&mut deny, cargo.join(name));
        }
    }
    // The system's keychains, outside the home and otherwise readable.
    #[cfg(target_os = "macos")]
    deny.push(PathBuf::from(SYSTEM_KEYCHAINS));
    deny.push(data_dir.to_path_buf());
    deny
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tree {
        _dir: tempfile::TempDir,
        root: PathBuf,
        home: PathBuf,
        data: PathBuf,
        project: PathBuf,
        scratch: PathBuf,
    }

    fn tree() -> Tree {
        let dir = tempfile::tempdir().unwrap();
        let root = resolved(dir.path());
        let home = root.join("home");
        let data = home
            .join("AppData")
            .join("Roaming")
            .join("com.shodh.rag-app");
        let project = home.join("code").join("project");
        let scratch = data.join(super::super::SCRATCH_DIR).join("abc");
        for folder in [
            &project,
            &scratch,
            &home.join(".ssh"),
            &home.join(".cargo").join("bin"),
        ] {
            std::fs::create_dir_all(folder).unwrap();
        }
        Tree {
            _dir: dir,
            root,
            home,
            data,
            project,
            scratch,
        }
    }

    fn policy(
        tree: &Tree,
        folder: &Path,
        env: &[(String, String)],
    ) -> Result<SandboxPolicy, String> {
        SandboxPolicy::for_command(PolicyInputs {
            folder,
            scratch: &tree.scratch,
            data_dir: &tree.data,
            home: Some(&tree.home),
            env,
            tools: &[],
        })
    }

    #[test]
    fn the_sandbox_policy_keeps_a_command_to_its_project_and_scratch_folder() {
        let tree = tree();
        let cargo = tree.home.join(".cargo");
        let path = std::env::join_paths([cargo.join("bin"), tree.root.join("elsewhere")]).unwrap();
        let env = vec![
            ("CARGO_HOME".to_string(), cargo.display().to_string()),
            ("PATH".to_string(), path.to_string_lossy().into_owned()),
        ];
        let policy = policy(&tree, &tree.project, &env).unwrap();
        assert_eq!(
            policy.writable,
            vec![tree.project.clone(), tree.scratch.clone()]
        );
        assert!(!policy.network);
        // The toolchain is readable; the PATH entry inside it adds nothing,
        // and one outside the home needs no grant.
        assert_eq!(policy.readable, vec![cargo.clone()]);

        assert!(policy.may_write(&tree.project.join("src").join("main.rs")));
        assert!(policy.may_write(&tree.scratch.join("tmp.txt")));
        assert!(!policy.may_write(&tree.home.join("notes.txt")));
        assert!(!policy.may_write(&cargo.join("bin").join("cargo")));
        assert!(!policy.may_write(&tree.data.join("index")));

        assert!(policy.may_read(&cargo.join("bin").join("cargo")));
        assert!(policy.may_read(&tree.root.join("elsewhere").join("tool")));
        assert!(!policy.may_read(&tree.home.join("Documents").join("taxes.pdf")));
        assert!(!policy.may_read(&tree.home.join(".ssh").join("id_ed25519")));
        // A credential inside a readable toolchain stays unreadable.
        assert!(!policy.may_read(&cargo.join("credentials.toml")));
        // Shodh's data is never readable, except the command's own scratch.
        assert!(!policy.may_read(&tree.data.join("audit.db")));
        assert!(!policy.may_read(&tree.data.join(super::super::SCRATCH_DIR).join("other")));
        assert!(policy.may_read(&tree.scratch.join("tmp.txt")));
    }

    #[test]
    fn the_sandbox_policy_refuses_folders_it_cannot_bound() {
        let tree = tree();
        for (folder, words) in [
            (tree.home.clone(), "home folder"),
            (tree.home.parent().unwrap().to_path_buf(), "home folder"),
            (tree.data.clone(), "Shodh's own data"),
            (
                tree.data.join(super::super::SCRATCH_DIR),
                "Shodh's own data",
            ),
            (tree.home.join(".ssh"), "credentials"),
        ] {
            let refused = policy(&tree, &folder, &[]).unwrap_err();
            assert!(refused.contains(words), "{}: {refused}", folder.display());
        }
        let root = tree.root.ancestors().last().unwrap().to_path_buf();
        assert!(policy(&tree, &root, &[])
            .unwrap_err()
            .contains("whole drive"));
        let missing = tree.home.join("gone");
        assert!(policy(&tree, &missing, &[])
            .unwrap_err()
            .contains("not found"));
    }

    #[test]
    fn a_credentials_folder_inside_the_project_stays_closed() {
        let tree = tree();
        // Opening ~/.config as the project must not open ~/.config/gh.
        let config = tree.home.join(".config");
        let gh = config.join("gh");
        std::fs::create_dir_all(&gh).unwrap();
        let policy = policy(&tree, &config, &[]).unwrap();
        let inside = policy.deny_inside_writable();
        assert!(inside.contains(&&gh), "{inside:?}");
        assert!(
            inside.iter().all(|deny| deny.starts_with(&config)),
            "{inside:?}"
        );
        assert!(!policy.may_read(&gh.join("hosts.yml")));
        assert!(!policy.may_write(&gh.join("hosts.yml")));
        assert!(policy.may_read(&config.join("other").join("settings.json")));
        assert!(policy.may_write(&config.join("other").join("settings.json")));
        // The scratch folder, inside Shodh's data, stays open.
        assert!(policy.may_write(&tree.scratch.join("tmp.txt")));
        assert!(!policy.may_read(&tree.data.join("audit.db")));
    }

    #[cfg(unix)]
    #[test]
    fn a_credentials_folder_that_is_a_link_stays_closed_where_it_points() {
        let tree = tree();
        let elsewhere = tree.root.join("elsewhere").join("aws");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, tree.home.join(".aws")).unwrap();
        let policy = policy(&tree, &tree.project, &[]).unwrap();
        assert!(policy.deny_read.contains(&tree.home.join(".aws")));
        assert!(
            policy.deny_read.contains(&elsewhere),
            "{:?}",
            policy.deny_read
        );
        // Outside the home, the target would otherwise be readable.
        assert!(!policy.may_read(&elsewhere.join("credentials")));
    }

    #[test]
    fn the_sandbox_policy_never_grants_reading_the_home_or_shodhs_data_through_path() {
        let tree = tree();
        let path =
            std::env::join_paths([tree.home.clone(), tree.data.clone(), tree.home.join(".ssh")])
                .unwrap();
        let env = vec![("PATH".to_string(), path.to_string_lossy().into_owned())];
        let policy = policy(&tree, &tree.project, &env).unwrap();
        assert!(policy.readable.is_empty(), "{:?}", policy.readable);
    }
}
