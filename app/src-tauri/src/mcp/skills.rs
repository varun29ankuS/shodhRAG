//! Installed Agent Skills: `<app data>/skills/<name>/SKILL.md`, with which
//! of them are on globally and per workspace in `<app data>/skills.json`.
//!
//! Installing is two steps, so the user sees what will be installed first:
//! [`SkillInstaller::prepare`] reads a local folder or clones a git URL into
//! a staging folder and lists the skills found; [`SkillInstaller::confirm`]
//! copies them in. A clone runs no hooks, and nothing a skill ships is ever
//! run. [`RECOMMENDED`] skills come from pinned commits of their repositories.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use shodh_rag::harness::mcp::Mode;
use shodh_rag::harness::skills::{find_skills, is_valid_name, Skill, SkillProblem};

use super::write_atomic;

/// Longest a `git clone` may take.
const CLONE_TIMEOUT: Duration = Duration::from_secs(180);
/// How deep a source is searched for skills.
const SEARCH_DEPTH: usize = 4;
/// Most files one installed skill may have.
const MAX_SKILL_FILES: usize = 2_000;
/// Largest installed skill.
const MAX_SKILL_BYTES: u64 = 50 * 1024 * 1024;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn skills_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("skills")
}

fn settings_path(data_dir: &Path) -> PathBuf {
    data_dir.join("skills.json")
}

/// A curated skill set, installed from a pinned commit of its repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Recommended {
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub repo: &'static str,
    pub commit: &'static str,
    pub license: &'static str,
    /// Skill folders of the repository that are installed (nothing else).
    pub folders: &'static [&'static str],
    /// Modes the skills are on in after installing (empty: every mode).
    pub modes: &'static [Mode],
}

/// The curated skills offered on the settings page (installed only when the
/// user asks).
pub const RECOMMENDED: [Recommended; 2] = [
    Recommended {
        id: "taste-skill",
        title: "Taste",
        description:
            "Design taste for front-end work: redesigns, minimalist layouts and polished output.",
        repo: "https://github.com/Leonxlnx/taste-skill",
        commit: "b482f7a970abb98c4108d4a9f761e458c64cefc8",
        license: "MIT",
        folders: &[
            "skills/taste-skill",
            "skills/redesign-skill",
            "skills/minimalist-skill",
            "skills/output-skill",
        ],
        modes: &[Mode::Code],
    },
    Recommended {
        id: "diagram-design",
        title: "Diagram design",
        description: "Clear, well-composed diagrams.",
        repo: "https://github.com/cathrynlavery/diagram-design",
        commit: "d1376371965f513d99cc9ec388835d255c5c88d5",
        license: "MIT",
        folders: &["skills/diagram-design"],
        modes: &[],
    },
];

pub fn recommended(id: &str) -> Option<&'static Recommended> {
    RECOMMENDED.iter().find(|r| r.id == id)
}

/// Where an installed skill came from (shown on its card).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillOrigin {
    pub license: String,
    pub repo: String,
}

/// Which skills are on: off everywhere (`disabled`), per workspace, and in
/// which modes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillSettings {
    #[serde(default)]
    pub disabled: Vec<String>,
    /// Workspace id → skill name → on.
    #[serde(default)]
    pub workspaces: BTreeMap<String, BTreeMap<String, bool>>,
    /// Skill name → the modes it is offered in (absent: every mode).
    #[serde(default)]
    pub modes: BTreeMap<String, Vec<Mode>>,
    /// Skill name → where it was installed from, when known.
    #[serde(default)]
    pub origins: BTreeMap<String, SkillOrigin>,
}

impl SkillSettings {
    /// Whether `name` is on in `workspace` (or globally): the workspace's
    /// own choice, else the global one (on unless disabled).
    pub fn enabled(&self, name: &str, workspace: Option<&str>) -> bool {
        workspace
            .and_then(|id| self.workspaces.get(id))
            .and_then(|w| w.get(name).copied())
            .unwrap_or_else(|| !self.disabled.iter().any(|d| d == name))
    }

    /// Whether `name` is offered in `mode`.
    pub fn in_mode(&self, name: &str, mode: Mode) -> bool {
        self.modes
            .get(name)
            .is_none_or(|modes| modes.is_empty() || modes.contains(&mode))
    }

    /// Set the modes `name` is offered in (empty or both: every mode).
    pub fn set_modes(&mut self, name: &str, modes: &[Mode]) {
        if modes.is_empty() || (modes.contains(&Mode::Research) && modes.contains(&Mode::Code)) {
            self.modes.remove(name);
        } else {
            self.modes.insert(name.to_string(), modes.to_vec());
        }
    }

    /// Turn `name` on or off in `workspace` (or globally).
    pub fn set(&mut self, name: &str, workspace: Option<&str>, enabled: bool) {
        match workspace {
            Some(id) => {
                self.workspaces
                    .entry(id.to_string())
                    .or_default()
                    .insert(name.to_string(), enabled);
            }
            None => {
                self.disabled.retain(|d| d != name);
                if !enabled {
                    self.disabled.push(name.to_string());
                }
            }
        }
    }
}

pub fn load_settings(data_dir: &Path) -> SkillSettings {
    std::fs::read_to_string(settings_path(data_dir))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

pub fn save_settings(data_dir: &Path, settings: &SkillSettings) -> Result<(), String> {
    let text = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    write_atomic(&settings_path(data_dir), &text).map_err(|e| e.to_string())
}

/// Every installed skill (and folders that are not readable skills).
pub fn installed(data_dir: &Path) -> (Vec<Skill>, Vec<SkillProblem>) {
    let dir = skills_dir(data_dir);
    if !dir.is_dir() {
        return (Vec::new(), Vec::new());
    }
    find_skills(&dir, 1)
}

/// The skills on in `workspace` (or globally) and `mode`.
pub fn enabled_skills(data_dir: &Path, workspace: Option<&str>, mode: Mode) -> Vec<Skill> {
    let settings = load_settings(data_dir);
    installed(data_dir)
        .0
        .into_iter()
        .filter(|s| settings.enabled(&s.name, workspace) && settings.in_mode(&s.name, mode))
        .collect()
}

/// Remove an installed skill; returns whether it existed.
pub fn remove(data_dir: &Path, name: &str) -> Result<bool, String> {
    if !is_valid_name(name) {
        return Err("invalid skill name".into());
    }
    let dir = skills_dir(data_dir).join(name);
    if !dir.is_dir() {
        return Ok(false);
    }
    std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut settings = load_settings(data_dir);
    if settings.origins.remove(name).is_some() | settings.modes.remove(name).is_some() {
        save_settings(data_dir, &settings)?;
    }
    Ok(true)
}

/// Where skills are installed from: a local folder, an `https` git URL or a
/// recommended set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillSource {
    Folder(PathBuf),
    Git(String),
    Recommended(&'static Recommended),
}

impl SkillSource {
    /// Read what the user typed or picked.
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        if text.is_empty() {
            return Err("Enter a folder or a git URL.".into());
        }
        if text.starts_with("https://") || text.starts_with("http://") || text.starts_with("git@") {
            let url = url::Url::parse(text).map_err(|_| "That is not a valid URL.".to_string())?;
            if url.scheme() != "https" || url.host_str().is_none_or(str::is_empty) {
                return Err("Only https:// git URLs can be installed.".into());
            }
            return Ok(SkillSource::Git(text.to_string()));
        }
        let path = PathBuf::from(text);
        if !path.is_dir() {
            return Err(format!("{text} is not a folder."));
        }
        Ok(SkillSource::Folder(path))
    }
}

/// Skills found in a source, waiting for the user's confirmation.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StagedInstall {
    pub token: String,
    pub source: String,
    pub skills: Vec<StagedSkill>,
    pub problems: Vec<SkillProblem>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StagedSkill {
    pub name: String,
    pub description: String,
    /// Files besides `SKILL.md`.
    pub files: usize,
    /// A skill of that name is installed and would be replaced.
    pub replaces: bool,
}

struct Staged {
    root: PathBuf,
    /// The root is a clone of ours, deleted when done.
    temporary: bool,
    skills: Vec<Skill>,
    /// A recommended set and its license file (kept with each skill).
    recommended: Option<(&'static Recommended, Option<PathBuf>)>,
}

/// Installs in progress, by token. Managed as Tauri state.
#[derive(Default)]
pub struct SkillInstaller {
    staged: Mutex<HashMap<String, Staged>>,
}

/// Run git with hooks, templates, symbolic links, prompts and local or
/// `ext::` transports off. Returns stdout.
async fn git(dest: &Path, args: &[&str]) -> Result<String, String> {
    let hooks = dest.with_extension("nohooks");
    let mut command = tokio::process::Command::new("git");
    command
        .arg("-c")
        .arg(format!("core.hooksPath={}", hooks.display()))
        .args([
            "-c",
            "core.symlinks=false",
            "-c",
            "protocol.file.allow=never",
            "-c",
            "protocol.ext.allow=never",
        ])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let output = tokio::time::timeout(CLONE_TIMEOUT, command.output())
        .await
        .map_err(|_| "Downloading took too long.".to_string())?
        .map_err(|e| format!("git could not be run ({e}). Is Git installed?"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let last = stderr
            .lines()
            .map(str::trim)
            .rfind(|l| !l.is_empty())
            .unwrap_or("git failed");
        Err(format!("Downloading failed: {last}"))
    }
}

/// Shallow clone of `url` into `dest`.
async fn clone(url: &str, dest: &Path) -> Result<(), String> {
    let dest_text = dest.display().to_string();
    git(
        dest,
        &[
            "clone",
            "--depth",
            "1",
            "--single-branch",
            "--no-tags",
            "--template=",
            "--",
            url,
            &dest_text,
        ],
    )
    .await
    .map(|_| ())
}

/// Only `commit` of `url`, checked out in `dest`.
async fn fetch_commit(url: &str, commit: &str, dest: &Path) -> Result<(), String> {
    let dest_text = dest.display().to_string();
    git(dest, &["init", "--quiet", "--template=", &dest_text]).await?;
    git(
        dest,
        &[
            "-C",
            &dest_text,
            "fetch",
            "--quiet",
            "--depth",
            "1",
            "--no-tags",
            url,
            commit,
        ],
    )
    .await?;
    git(
        dest,
        &[
            "-C",
            &dest_text,
            "checkout",
            "--quiet",
            "--detach",
            "FETCH_HEAD",
        ],
    )
    .await?;
    let head = git(dest, &["-C", &dest_text, "rev-parse", "HEAD"]).await?;
    if head != commit {
        return Err(format!(
            "the repository returned {head}, not the pinned {commit}"
        ));
    }
    Ok(())
}

/// The repository's license file, if it has one at its root.
fn license_file(root: &Path) -> Option<PathBuf> {
    ["LICENSE", "LICENSE.md", "LICENSE.txt", "LICENCE"]
        .iter()
        .map(|name| root.join(name))
        .find(|p| p.is_file())
}

/// Size and file count of a folder (symbolic links are not followed).
fn measure(dir: &Path) -> (usize, u64) {
    walkdir::WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| e.file_name() != ".git")
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .fold((0, 0), |(n, size), e| {
            (n + 1, size + e.metadata().map(|m| m.len()).unwrap_or(0))
        })
}

/// Copy `from` into `to` (regular files and folders only; no `.git`).
fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    for entry in walkdir::WalkDir::new(from)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| e.file_name() != ".git")
    {
        let entry = entry.map_err(std::io::Error::other)?;
        let rel = entry
            .path()
            .strip_prefix(from)
            .map_err(std::io::Error::other)?;
        let target = to.join(rel);
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&target)?;
        } else if entry.file_type().is_file() {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

impl SkillInstaller {
    /// Find the skills in `source` (cloning a git URL first).
    pub async fn prepare(
        &self,
        data_dir: &Path,
        source: SkillSource,
    ) -> Result<StagedInstall, String> {
        let token = uuid::Uuid::new_v4().simple().to_string();
        let pinned = match &source {
            SkillSource::Recommended(set) => Some(*set),
            _ => None,
        };
        let (root, temporary, shown) = match source {
            SkillSource::Folder(path) => {
                let shown = path.display().to_string();
                (path, false, shown)
            }
            SkillSource::Recommended(set) => {
                let dest = data_dir.join("skills-staging").join(&token);
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                if let Err(e) = fetch_commit(set.repo, set.commit, &dest).await {
                    let _ = std::fs::remove_dir_all(&dest);
                    return Err(e);
                }
                (dest, true, format!("{} at {}", set.repo, &set.commit[..12]))
            }
            SkillSource::Git(url) => {
                let dest = data_dir.join("skills-staging").join(&token);
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                if let Err(e) = clone(&url, &dest).await {
                    let _ = std::fs::remove_dir_all(&dest);
                    return Err(e);
                }
                (dest, true, url)
            }
        };
        let scan_root = root.clone();
        let (mut skills, problems) =
            tokio::task::spawn_blocking(move || find_skills(&scan_root, SEARCH_DEPTH))
                .await
                .map_err(|e| e.to_string())?;
        // A recommended set installs only its listed folders.
        if let Some(set) = pinned {
            skills.retain(|s| set.folders.iter().any(|f| s.dir == root.join(f)));
        }
        if skills.is_empty() {
            if temporary {
                let _ = std::fs::remove_dir_all(&root);
            }
            let detail = problems
                .first()
                .map(|p| format!(" ({}: {})", p.folder, p.error))
                .unwrap_or_default();
            return Err(format!(
                "No skill (a folder with a SKILL.md) was found there{detail}."
            ));
        }
        let installed: Vec<String> = installed(data_dir).0.into_iter().map(|s| s.name).collect();
        let staged = StagedInstall {
            token: token.clone(),
            source: shown,
            skills: skills
                .iter()
                .map(|s| StagedSkill {
                    name: s.name.clone(),
                    description: s.description.clone(),
                    files: shodh_rag::harness::skills::resource_files(&s.dir).len(),
                    replaces: installed.contains(&s.name),
                })
                .collect(),
            problems,
        };
        lock(&self.staged).insert(
            token,
            Staged {
                recommended: pinned.map(|set| (set, license_file(&root))),
                root,
                temporary,
                skills,
            },
        );
        Ok(staged)
    }

    /// Install what `token` staged; returns the installed names.
    pub async fn confirm(&self, data_dir: &Path, token: &str) -> Result<Vec<String>, String> {
        let staged = lock(&self.staged)
            .remove(token)
            .ok_or("This install is no longer pending; start it again.")?;
        let target_root = skills_dir(data_dir);
        let data_dir = data_dir.to_path_buf();
        let result = tokio::task::spawn_blocking(move || -> Result<Vec<String>, String> {
            std::fs::create_dir_all(&target_root).map_err(|e| e.to_string())?;
            let mut names = Vec::new();
            for skill in &staged.skills {
                let (files, bytes) = measure(&skill.dir);
                if files > MAX_SKILL_FILES || bytes > MAX_SKILL_BYTES {
                    return Err(format!(
                        "{} is too large to install ({files} files, {} MB)",
                        skill.name,
                        bytes / (1024 * 1024)
                    ));
                }
            }
            for skill in &staged.skills {
                let target = target_root.join(&skill.name);
                let incoming = target_root.join(format!(".{}.incoming", skill.name));
                let _ = std::fs::remove_dir_all(&incoming);
                copy_tree(&skill.dir, &incoming).map_err(|e| format!("{}: {e}", skill.name))?;
                // The repository's license travels with each of its skills.
                if let Some((_, Some(license))) = &staged.recommended {
                    let kept = incoming.join(license.file_name().unwrap_or_default());
                    if !kept.exists() {
                        std::fs::copy(license, &kept)
                            .map_err(|e| format!("{}: {e}", skill.name))?;
                    }
                }
                if target.exists() {
                    std::fs::remove_dir_all(&target).map_err(|e| format!("{}: {e}", skill.name))?;
                }
                std::fs::rename(&incoming, &target).map_err(|e| format!("{}: {e}", skill.name))?;
                names.push(skill.name.clone());
            }
            if let Some((set, _)) = staged.recommended {
                let mut settings = load_settings(&data_dir);
                for name in &names {
                    settings.set_modes(name, set.modes);
                    settings.origins.insert(
                        name.clone(),
                        SkillOrigin {
                            license: set.license.to_string(),
                            repo: set.repo.to_string(),
                        },
                    );
                }
                save_settings(&data_dir, &settings)?;
            }
            if staged.temporary {
                let _ = std::fs::remove_dir_all(&staged.root);
            }
            Ok(names)
        })
        .await
        .map_err(|e| e.to_string())?;
        result
    }

    /// Drop what `token` staged.
    pub fn cancel(&self, token: &str) {
        if let Some(staged) = lock(&self.staged).remove(token) {
            if staged.temporary {
                let _ = std::fs::remove_dir_all(&staged.root);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_skill(root: &Path, name: &str) {
        let dir = root.join(name);
        std::fs::create_dir_all(dir.join("scripts")).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: Does {name}.\n---\nSteps."),
        )
        .unwrap();
        std::fs::write(dir.join("scripts").join("tool.py"), "print(1)").unwrap();
    }

    #[test]
    fn skills_are_on_globally_unless_a_workspace_says_otherwise() {
        let mut settings = SkillSettings::default();
        assert!(settings.enabled("pdf", None));
        settings.set("pdf", None, false);
        assert!(!settings.enabled("pdf", None));
        assert!(!settings.enabled("pdf", Some("ws1")));
        settings.set("pdf", Some("ws1"), true);
        assert!(settings.enabled("pdf", Some("ws1")));
        assert!(!settings.enabled("pdf", Some("ws2")));
        settings.set("pdf", None, true);
        assert_eq!(settings.disabled, Vec::<String>::new());
    }

    #[test]
    fn sources_are_folders_or_https_git_urls() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            SkillSource::parse(&dir.path().display().to_string()).unwrap(),
            SkillSource::Folder(dir.path().to_path_buf())
        );
        assert_eq!(
            SkillSource::parse("https://github.com/anthropics/skills").unwrap(),
            SkillSource::Git("https://github.com/anthropics/skills".into())
        );
        assert!(SkillSource::parse("http://example.com/x.git").is_err());
        assert!(SkillSource::parse("git@github.com:a/b.git").is_err());
        assert!(SkillSource::parse("C:/no/such/folder").is_err());
        assert!(SkillSource::parse(" ").is_err());
    }

    #[tokio::test]
    async fn folders_are_previewed_then_installed_and_toggled_per_workspace() {
        let data = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        write_skill(&source.path().join("skills"), "alpha");
        write_skill(&source.path().join("skills"), "beta");
        let installer = SkillInstaller::default();
        let staged = installer
            .prepare(
                data.path(),
                SkillSource::Folder(source.path().to_path_buf()),
            )
            .await
            .unwrap();
        let names: Vec<&str> = staged.skills.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["alpha", "beta"]);
        assert_eq!(staged.skills[0].files, 1);
        assert!(
            installed(data.path()).0.is_empty(),
            "nothing before confirming"
        );
        let done = installer.confirm(data.path(), &staged.token).await.unwrap();
        assert_eq!(done, ["alpha", "beta"]);
        assert!(installer.confirm(data.path(), &staged.token).await.is_err());
        assert!(skills_dir(data.path())
            .join("alpha/scripts/tool.py")
            .is_file());
        assert!(
            source.path().join("skills/alpha/SKILL.md").is_file(),
            "source kept"
        );

        let mut settings = load_settings(data.path());
        settings.set("beta", Some("ws1"), false);
        save_settings(data.path(), &settings).unwrap();
        let on = |ws: Option<&str>| -> Vec<String> {
            enabled_skills(data.path(), ws, Mode::Research)
                .into_iter()
                .map(|s| s.name)
                .collect()
        };
        assert_eq!(on(None), ["alpha", "beta"]);
        assert_eq!(on(Some("ws1")), ["alpha"]);
        // Installing again shows what would be replaced.
        let again = installer
            .prepare(
                data.path(),
                SkillSource::Folder(source.path().join("skills/alpha")),
            )
            .await
            .unwrap();
        assert!(again.skills[0].replaces);
        installer.cancel(&again.token);
        assert!(remove(data.path(), "alpha").unwrap());
        assert!(!remove(data.path(), "alpha").unwrap());
        assert!(remove(data.path(), "../x").is_err());
        assert_eq!(on(None), ["beta"]);
    }

    #[test]
    fn skills_can_be_limited_to_a_mode() {
        let mut settings = SkillSettings::default();
        assert!(settings.in_mode("taste-skill", Mode::Research));
        settings.set_modes("taste-skill", &[Mode::Code]);
        assert!(settings.in_mode("taste-skill", Mode::Code));
        assert!(!settings.in_mode("taste-skill", Mode::Research));
        settings.set_modes("taste-skill", &[Mode::Research, Mode::Code]);
        assert!(settings.modes.is_empty());
    }

    #[test]
    fn recommended_sets_are_pinned_and_limited_to_their_folders() {
        for set in RECOMMENDED {
            assert_eq!(set.commit.len(), 40, "{}", set.id);
            assert!(set.repo.starts_with("https://github.com/"));
            assert_eq!(set.license, "MIT");
            assert!(SkillSource::parse(set.repo).is_ok());
            assert!(set.folders.iter().all(|f| f.starts_with("skills/")));
        }
        let taste = recommended("taste-skill").unwrap();
        assert_eq!(taste.modes, [Mode::Code]);
        assert!(!taste.folders.iter().any(|f| f.contains("imagegen")
            || f.contains("brandkit")
            || f.contains("image-to-code")));
        assert!(recommended("diagram-design").unwrap().modes.is_empty());
        assert!(recommended("other").is_none());
    }

    /// Against GitHub: the pinned commits exist and hold the listed skills.
    #[tokio::test]
    #[ignore = "downloads from github.com"]
    async fn recommended_sets_install_from_their_pinned_commits() {
        let data = tempfile::tempdir().unwrap();
        let installer = SkillInstaller::default();
        for set in RECOMMENDED.iter() {
            let staged = installer
                .prepare(data.path(), SkillSource::Recommended(set))
                .await
                .unwrap();
            assert_eq!(staged.skills.len(), set.folders.len(), "{}", set.id);
            let names = installer.confirm(data.path(), &staged.token).await.unwrap();
            for name in &names {
                let dir = skills_dir(data.path()).join(name);
                assert!(dir.join("SKILL.md").is_file());
                assert!(license_file(&dir).is_some(), "{name} keeps the license");
            }
        }
        let settings = load_settings(data.path());
        assert!(!settings.in_mode("design-taste-frontend", Mode::Research));
        assert_eq!(settings.origins["diagram-design"].license, "MIT");
    }

    #[tokio::test]
    async fn a_folder_without_skills_is_refused() {
        let data = tempfile::tempdir().unwrap();
        let empty = tempfile::tempdir().unwrap();
        let error = SkillInstaller::default()
            .prepare(data.path(), SkillSource::Folder(empty.path().to_path_buf()))
            .await
            .unwrap_err();
        assert!(error.contains("No skill"), "{error}");
    }
}
