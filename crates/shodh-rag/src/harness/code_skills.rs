//! Shodh's bundled building skills for Code mode: plan first, test first,
//! debug systematically, verify before done, and a fresh-eyes review.
//!
//! How they load: omp runs with `--no-skills` and omp's own skill settings
//! off ([`super::sidecar`]), so omp never reads a skill folder by itself.
//! Every skill, installed or bundled, reaches the model through Shodh's host
//! tools [`super::skills::LoadSkillTool`] and
//! [`super::skills::ReadSkillFileTool`] in the open `SKILL.md` format: the
//! model sees each skill's name and description on every turn and loads the
//! instructions when a task calls for them. The bundled skills are compiled
//! into Shodh ([`BUNDLED`]) and written to `<app data>/code-skills/` when a
//! Code session starts ([`materialize`]), so those tools read them like any
//! other skill; the user turns each on or off in Settings → Code.
//!
//! The set is one merged framework, not several side by side: each skill
//! was rewritten from obra/superpowers (MIT), affaan-m/ECC (MIT) and
//! google/elephant-goldfish-model (Apache-2.0) with their conflicts resolved
//! (written in each skill's "Sources and credit"), and every skill names the
//! others in one order. Their licence texts travel with each skill. No skill
//! changes Shodh's mode or approvals or tells the agent to get around them
//! ([`ESCALATION_PHRASES`] is checked by the tests), and none ships a script.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::Serialize;

use super::skills::{read_skill, Skill, SKILL_FILE};

/// Where the bundled skills are written, inside Shodh's data folder.
pub const DIR: &str = "code-skills";

/// A project a bundled skill was written from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    pub project: &'static str,
    pub url: &'static str,
    pub commit: &'static str,
    pub license: &'static str,
    /// The project's skills it draws on.
    pub skills: &'static [&'static str],
    /// The licence file kept beside the skill.
    #[serde(skip)]
    pub license_file: &'static str,
    #[serde(skip)]
    pub license_text: &'static str,
}

/// One bundled skill.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BundledSkill {
    /// Its skill name (the folder and the `name` of its `SKILL.md`).
    pub name: &'static str,
    /// What Settings calls it.
    pub title: &'static str,
    /// What it makes the agent do, in plain words, for Settings.
    pub summary: &'static str,
    pub sources: &'static [Source],
    #[serde(skip)]
    pub skill_md: &'static str,
}

const SUPERPOWERS_LICENSE: &str = include_str!("../../skills/licenses/superpowers-MIT.txt");
const ECC_LICENSE: &str = include_str!("../../skills/licenses/ECC-MIT.txt");
const EGM_LICENSE: &str =
    include_str!("../../skills/licenses/elephant-goldfish-model-Apache-2.0.txt");

const SUPERPOWERS: &str = "obra/superpowers";
const SUPERPOWERS_URL: &str = "https://github.com/obra/superpowers";
const SUPERPOWERS_COMMIT: &str = "22f0b463e7a5e579904dd6211a520321035526f7";
const SUPERPOWERS_FILE: &str = "LICENSE-superpowers.txt";
const ECC: &str = "affaan-m/ECC";
const ECC_URL: &str = "https://github.com/affaan-m/ECC";
const ECC_COMMIT: &str = "d2ef2680594f9a17526a6e1a2e18b3d242c5d17b";
const ECC_FILE: &str = "LICENSE-ECC.txt";
const EGM: &str = "google/elephant-goldfish-model";
const EGM_URL: &str = "https://github.com/google/elephant-goldfish-model";
const EGM_COMMIT: &str = "e9e1f06aa4e26e95cd4ed37210a0632202c429e9";
const EGM_FILE: &str = "LICENSE-elephant-goldfish-model.txt";

const fn superpowers(skills: &'static [&'static str]) -> Source {
    Source {
        project: SUPERPOWERS,
        url: SUPERPOWERS_URL,
        commit: SUPERPOWERS_COMMIT,
        license: "MIT",
        skills,
        license_file: SUPERPOWERS_FILE,
        license_text: SUPERPOWERS_LICENSE,
    }
}

const fn ecc(skills: &'static [&'static str]) -> Source {
    Source {
        project: ECC,
        url: ECC_URL,
        commit: ECC_COMMIT,
        license: "MIT",
        skills,
        license_file: ECC_FILE,
        license_text: ECC_LICENSE,
    }
}

const fn egm(skills: &'static [&'static str]) -> Source {
    Source {
        project: EGM,
        url: EGM_URL,
        commit: EGM_COMMIT,
        license: "Apache-2.0",
        skills,
        license_file: EGM_FILE,
        license_text: EGM_LICENSE,
    }
}

/// The bundled skills, in the order they are used.
pub const BUNDLED: [BundledSkill; 5] = [
    BundledSkill {
        name: "plan-before-building",
        title: "Plan before building",
        summary: "Asks what you want, looks for code that already does it, weighs two or three \
                  approaches and agrees a short plan with you before writing code.",
        sources: &[
            superpowers(&["brainstorming", "writing-plans"]),
            ecc(&["search-first"]),
            egm(&["egm-brainstorm", "egm-draft"]),
        ],
        skill_md: include_str!("../../skills/plan-before-building/SKILL.md"),
    },
    BundledSkill {
        name: "test-first",
        title: "Test first",
        summary: "Writes a failing test before the code, checks it fails for the right reason, \
                  then writes the least code that passes.",
        sources: &[
            superpowers(&["test-driven-development"]),
            ecc(&["tdd-workflow"]),
        ],
        skill_md: include_str!("../../skills/test-first/SKILL.md"),
    },
    BundledSkill {
        name: "debug-systematically",
        title: "Debug systematically",
        summary: "Reads the whole error, reproduces it and finds the cause before fixing \
                  anything; stops and asks you after three fixes that did not work.",
        sources: &[superpowers(&["systematic-debugging"])],
        skill_md: include_str!("../../skills/debug-systematically/SKILL.md"),
    },
    BundledSkill {
        name: "verify-before-done",
        title: "Verify before done",
        summary: "Runs the build, the checks and the tests before saying work is done, and \
                  reports what passed, what failed and what was not checked.",
        sources: &[
            superpowers(&["verification-before-completion"]),
            ecc(&["verification-loop"]),
        ],
        skill_md: include_str!("../../skills/verify-before-done/SKILL.md"),
    },
    BundledSkill {
        name: "fresh-eyes-review",
        title: "Fresh-eyes review",
        summary: "Has Shodh's reviewer, which never sees the conversation, check the changed \
                  code, then checks each finding against the code before acting on it.",
        sources: &[
            superpowers(&["requesting-code-review", "receiving-code-review"]),
            egm(&["egm-review", "egm-resolve"]),
        ],
        skill_md: include_str!("../../skills/fresh-eyes-review/SKILL.md"),
    },
];

/// Words that would make a skill change Shodh's mode or approvals, or get
/// around them. No bundled skill contains one (checked by the tests).
pub const ESCALATION_PHRASES: [&str; 16] = [
    "bypass",
    "--no-verify",
    "auto mode",
    "switch mode",
    "change mode",
    "change the mode",
    "suggest_mode",
    "without approval",
    "without asking",
    "skip approval",
    "approve it yourself",
    "allowlist",
    "dangerously",
    "sudo ",
    "rm -rf",
    "git push",
];

/// The bundled skill named `name`.
pub fn bundled(name: &str) -> Option<&'static BundledSkill> {
    BUNDLED.iter().find(|s| s.name == name)
}

/// Whether `name` is a bundled skill's (an installed skill of that name is
/// not offered in Code mode: the bundled one is).
pub fn is_bundled(name: &str) -> bool {
    bundled(name).is_some()
}

/// The folder the bundled skills are written to.
pub fn dir(data_dir: &Path) -> PathBuf {
    data_dir.join(DIR)
}

/// The files of `skill`: its `SKILL.md` and its sources' licences.
fn files(skill: &BundledSkill) -> Vec<(&'static str, &'static str)> {
    let mut files = vec![(SKILL_FILE, skill.skill_md)];
    for source in skill.sources {
        if !files.iter().any(|(name, _)| *name == source.license_file) {
            files.push((source.license_file, source.license_text));
        }
    }
    files
}

/// Write `text` to `path` unless it already holds exactly that.
fn write_if_changed(path: &Path, text: &str) -> Result<(), String> {
    if std::fs::read(path).is_ok_and(|current| current == text.as_bytes()) {
        return Ok(());
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Write the bundled skills into `<data_dir>/code-skills/`, each in its own
/// folder with its licences, and remove anything else there (a skill or file
/// an earlier version bundled). Returns the skills as the skill tools read
/// them, in [`BUNDLED`] order.
pub fn materialize(data_dir: &Path) -> Result<Vec<Skill>, String> {
    // Sessions starting together write the same files one at a time.
    static WRITING: Mutex<()> = Mutex::new(());
    let _one = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    let root = dir(data_dir);
    std::fs::create_dir_all(&root).map_err(|e| format!("{}: {e}", root.display()))?;
    // Folders and files of earlier versions go.
    for entry in std::fs::read_dir(&root).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !is_bundled(&name) {
            let path = entry.path();
            let removed = if path.is_dir() {
                std::fs::remove_dir_all(&path)
            } else {
                std::fs::remove_file(&path)
            };
            removed.map_err(|e| format!("{}: {e}", path.display()))?;
        }
    }
    let mut skills = Vec::with_capacity(BUNDLED.len());
    for skill in &BUNDLED {
        let folder = root.join(skill.name);
        std::fs::create_dir_all(&folder).map_err(|e| format!("{}: {e}", folder.display()))?;
        let wanted = files(skill);
        for entry in std::fs::read_dir(&folder).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if !wanted.iter().any(|(file, _)| *file == name) {
                let path = entry.path();
                let removed = if path.is_dir() {
                    std::fs::remove_dir_all(&path)
                } else {
                    std::fs::remove_file(&path)
                };
                removed.map_err(|e| format!("{}: {e}", path.display()))?;
            }
        }
        for (file, text) in wanted {
            write_if_changed(&folder.join(file), text)?;
        }
        let (read, _) = read_skill(&folder)?;
        skills.push(read);
    }
    Ok(skills)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::skills::{parse_skill_md, resource_files};

    #[test]
    fn every_bundled_skill_is_a_valid_skill_named_like_its_folder() {
        for skill in &BUNDLED {
            let doc =
                parse_skill_md(skill.skill_md).unwrap_or_else(|e| panic!("{}: {e}", skill.name));
            assert_eq!(doc.name, skill.name);
            assert!(doc.description.starts_with("Use "), "{}", skill.name);
            assert!(!skill.sources.is_empty());
            assert!(skill.summary.len() > 40 && skill.title.len() > 3);
        }
        let mut names: Vec<&str> = BUNDLED.iter().map(|s| s.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), BUNDLED.len());
    }

    #[test]
    fn every_skill_credits_its_sources_and_keeps_their_licences() {
        for skill in &BUNDLED {
            let body = parse_skill_md(skill.skill_md).unwrap().body;
            let credit = body
                .split("## Sources and credit")
                .nth(1)
                .unwrap_or_else(|| panic!("{} has no Sources and credit section", skill.name));
            for source in skill.sources {
                assert!(
                    credit.contains(source.project),
                    "{}: {}",
                    skill.name,
                    source.project
                );
                assert!(
                    credit.contains(source.url),
                    "{}: {}",
                    skill.name,
                    source.url
                );
                assert!(
                    credit.contains(source.commit),
                    "{}: {}",
                    skill.name,
                    source.commit
                );
                assert!(
                    credit.contains(source.license_file),
                    "{}: {}",
                    skill.name,
                    source.license_file
                );
                for name in source.skills {
                    assert!(credit.contains(name), "{}: {name}", skill.name);
                }
                match source.license {
                    "MIT" => {
                        assert!(source.license_text.starts_with("MIT License"));
                        assert!(source.license_text.contains("Copyright (c)"));
                    }
                    "Apache-2.0" => {
                        assert!(source.license_text.contains("Apache License"));
                        assert!(source.license_text.contains("Version 2.0, January 2004"));
                        // Apache-2.0 §4(b): the changes are stated.
                        assert!(credit.contains("Changed by Shodh"), "{}", skill.name);
                    }
                    other => panic!("{}: unexpected licence {other}", skill.name),
                }
            }
        }
    }

    #[test]
    fn no_skill_changes_modes_or_approvals_or_ships_a_script() {
        for skill in &BUNDLED {
            let lowered = skill.skill_md.to_lowercase();
            for phrase in ESCALATION_PHRASES {
                assert!(!lowered.contains(phrase), "{} says {phrase:?}", skill.name);
            }
            for (file, _) in files(skill) {
                assert!(
                    file == SKILL_FILE || (file.starts_with("LICENSE-") && file.ends_with(".txt")),
                    "{}: {file}",
                    skill.name
                );
            }
        }
    }

    #[test]
    fn every_skill_names_the_others_in_one_order() {
        let order: Vec<&str> = BUNDLED.iter().map(|s| s.name).collect();
        for skill in &BUNDLED {
            let body = parse_skill_md(skill.skill_md).unwrap().body;
            let mut at = 0;
            for name in &order {
                let found = body[at..]
                    .find(name)
                    .unwrap_or_else(|| panic!("{} does not name {name} in order", skill.name));
                at += found + name.len();
            }
        }
    }

    #[test]
    fn the_skills_are_written_once_and_stale_files_removed() {
        let data = tempfile::tempdir().unwrap();
        let root = dir(data.path());
        std::fs::create_dir_all(root.join("old-skill")).unwrap();
        std::fs::write(root.join("old-skill").join(SKILL_FILE), "old").unwrap();
        std::fs::create_dir_all(root.join("test-first")).unwrap();
        std::fs::write(root.join("test-first").join("run.sh"), "echo hi").unwrap();
        std::fs::write(root.join("test-first").join(SKILL_FILE), "edited").unwrap();

        let skills = materialize(data.path()).unwrap();
        let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        let order: Vec<&str> = BUNDLED.iter().map(|s| s.name).collect();
        assert_eq!(names, order);
        assert!(!root.join("old-skill").exists());
        assert!(!root.join("test-first").join("run.sh").exists());
        assert_eq!(
            std::fs::read_to_string(root.join("test-first").join(SKILL_FILE)).unwrap(),
            bundled("test-first").unwrap().skill_md
        );
        assert_eq!(
            resource_files(&root.join("fresh-eyes-review")),
            [
                "LICENSE-elephant-goldfish-model.txt",
                "LICENSE-superpowers.txt"
            ]
        );
        assert_eq!(
            resource_files(&root.join("debug-systematically")),
            ["LICENSE-superpowers.txt"]
        );
        // Again: nothing to change, the same skills.
        assert_eq!(materialize(data.path()).unwrap(), skills);
    }
}
