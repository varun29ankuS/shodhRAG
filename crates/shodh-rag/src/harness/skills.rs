//! Agent Skills (the open `SKILL.md` format): a folder with a `SKILL.md`
//! whose YAML front matter names the skill and says when to use it, whose
//! body holds the instructions, and optional resource files beside it.
//!
//! The agent sees only each skill's name and description (in
//! [`LOAD_SKILL`]'s description, so every turn has them). `load_skill`
//! returns a skill's instructions and lists its files; `read_skill_file`
//! reads one of them, confined to that skill's folder. Nothing a skill ships
//! is ever run.

use std::path::{Component, Path, PathBuf};

use async_trait::async_trait;
use serde::Serialize;
use serde_json::{json, Value};

use super::events::RiskTier;
use super::protocol::ToolLoadMode;
use super::tools::{req_str, HostTool, ToolContext, ToolError, ToolOutput};
use super::truncate_chars;

pub const LOAD_SKILL: &str = "load_skill";
pub const READ_SKILL_FILE: &str = "read_skill_file";

/// The file that makes a folder a skill.
pub const SKILL_FILE: &str = "SKILL.md";

/// Longest skill name (the format's limit).
pub const MAX_NAME_CHARS: usize = 64;
/// Longest description (the format's limit).
pub const MAX_DESCRIPTION_CHARS: usize = 1_024;
/// Largest `SKILL.md` or resource file read.
pub const MAX_FILE_BYTES: u64 = 512 * 1024;
/// Most resource files listed for one skill.
const MAX_LISTED_FILES: usize = 200;
/// How deep resource files are listed.
const MAX_LIST_DEPTH: usize = 4;

/// A skill's front matter and instructions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillDoc {
    pub name: String,
    pub description: String,
    pub body: String,
}

/// An installed skill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Skill {
    pub name: String,
    pub description: String,
    /// The skill's folder.
    pub dir: PathBuf,
}

/// Whether `name` is a valid skill name: 1 to 64 lowercase letters, digits
/// and hyphens, not starting or ending with a hyphen and without `--`.
pub fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= MAX_NAME_CHARS
        && !name.starts_with('-')
        && !name.ends_with('-')
        && !name.contains("--")
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// A YAML scalar as written on one line: quotes removed (with their escapes).
fn unquote(value: &str) -> String {
    let value = value.trim();
    let mut chars = value.char_indices();
    match chars.next() {
        // A double-quoted scalar ends at the first unescaped quote.
        Some((_, '"')) => {
            let mut out = String::new();
            let mut escaped = false;
            for (_, c) in chars {
                match (escaped, c) {
                    (false, '\\') => escaped = true,
                    (false, '"') => return out,
                    (true, 'n') => {
                        out.push('\n');
                        escaped = false;
                    }
                    (_, c) => {
                        out.push(c);
                        escaped = false;
                    }
                }
            }
            out
        }
        // A single-quoted scalar ends at a quote that is not doubled.
        Some((_, '\'')) => {
            let mut out = String::new();
            let mut rest = value[1..].chars().peekable();
            while let Some(c) = rest.next() {
                if c == '\'' {
                    if rest.peek() == Some(&'\'') {
                        rest.next();
                    } else {
                        return out;
                    }
                }
                out.push(c);
            }
            out
        }
        // A plain scalar, without a trailing comment.
        _ => match value.find(" #") {
            Some(at) => value[..at].trim_end().to_string(),
            None => value.to_string(),
        },
    }
}

/// The top-level scalar `key` of a YAML front matter: plain, quoted, a
/// block scalar (`|`, `>`) or a plain scalar continued on indented lines.
fn front_matter_value(lines: &[&str], key: &str) -> Option<String> {
    let prefix = format!("{key}:");
    let start = lines.iter().position(|l| l.starts_with(&prefix))?;
    let first = lines[start][prefix.len()..].trim();
    let continuation: Vec<&str> = lines[start + 1..]
        .iter()
        .take_while(|l| l.trim().is_empty() || l.starts_with(' ') || l.starts_with('\t'))
        .map(|l| l.trim())
        .collect();
    let folded = |parts: &[&str]| {
        parts
            .iter()
            .filter(|p| !p.is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join(" ")
    };
    let value = if first.starts_with('|') {
        continuation.join("\n").trim().to_string()
    } else if first.starts_with('>') {
        folded(&continuation)
    } else if continuation.iter().any(|l| !l.is_empty()) && !first.starts_with(['"', '\'']) {
        let mut parts = vec![first];
        parts.extend(continuation.iter().copied());
        folded(&parts)
    } else {
        unquote(first)
    };
    Some(value)
}

/// Parse a `SKILL.md`.
pub fn parse_skill_md(text: &str) -> Result<SkillDoc, String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.lines();
    if lines.next().map(str::trim_end) != Some("---") {
        return Err("SKILL.md must start with front matter (a line with ---)".into());
    }
    let mut front: Vec<&str> = Vec::new();
    let mut closed = false;
    for line in lines.by_ref() {
        if line.trim_end() == "---" {
            closed = true;
            break;
        }
        front.push(line);
    }
    if !closed {
        return Err("the front matter is not closed with ---".into());
    }
    let body = lines.collect::<Vec<_>>().join("\n").trim().to_string();
    let name = front_matter_value(&front, "name")
        .filter(|n| !n.is_empty())
        .ok_or("the front matter has no name")?;
    if !is_valid_name(&name) {
        return Err(format!(
            "\"{name}\" is not a valid skill name (lowercase letters, digits and hyphens)"
        ));
    }
    let description = front_matter_value(&front, "description")
        .map(|d| d.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|d| !d.is_empty())
        .ok_or("the front matter has no description")?;
    if description.chars().count() > MAX_DESCRIPTION_CHARS {
        return Err(format!(
            "the description is longer than {MAX_DESCRIPTION_CHARS} characters"
        ));
    }
    Ok(SkillDoc {
        name,
        description,
        body,
    })
}

/// Read a file of at most [`MAX_FILE_BYTES`] as text.
fn read_text(path: &Path) -> Result<String, String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if meta.len() > MAX_FILE_BYTES {
        return Err(format!("larger than {} KB", MAX_FILE_BYTES / 1024));
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    String::from_utf8(bytes).map_err(|_| "not a text file".to_string())
}

/// Read the skill in `dir`.
pub fn read_skill(dir: &Path) -> Result<(Skill, String), String> {
    let text = read_text(&dir.join(SKILL_FILE))?;
    let doc = parse_skill_md(&text)?;
    Ok((
        Skill {
            name: doc.name,
            description: doc.description,
            dir: dir.to_path_buf(),
        },
        doc.body,
    ))
}

/// A folder that could not be read as a skill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SkillProblem {
    pub folder: String,
    pub error: String,
}

/// Every folder at most `depth` levels below `root` (and `root` itself)
/// holding a `SKILL.md`, read as skills, sorted by name. Symbolic links and
/// `.git` are not followed.
pub fn find_skills(root: &Path, depth: usize) -> (Vec<Skill>, Vec<SkillProblem>) {
    let mut skills = Vec::new();
    let mut problems = Vec::new();
    for entry in walkdir::WalkDir::new(root)
        .max_depth(depth + 1)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| e.file_name() != ".git" && e.file_name() != "node_modules")
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_file() || entry.file_name() != SKILL_FILE {
            continue;
        }
        let Some(dir) = entry.path().parent() else {
            continue;
        };
        match read_skill(dir) {
            Ok((skill, _)) => skills.push(skill),
            Err(error) => problems.push(SkillProblem {
                folder: dir.display().to_string(),
                error,
            }),
        }
    }
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    (skills, problems)
}

/// `relative` inside `root`, or why it may not be read: absolute paths,
/// drive or UNC prefixes, `..`, `:` (alternate data streams) and anything
/// resolving (through links) outside `root` are refused.
pub fn confined_path(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let relative = relative.trim();
    if relative.is_empty() {
        return Err("a file path is needed".into());
    }
    if relative.contains(':') || relative.starts_with(['/', '\\']) {
        return Err(format!("\"{relative}\" is not a path inside the skill"));
    }
    let path = Path::new(relative);
    if path
        .components()
        .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        return Err(format!("\"{relative}\" is not a path inside the skill"));
    }
    let root = std::fs::canonicalize(root).map_err(|e| format!("the skill folder: {e}"))?;
    let target = std::fs::canonicalize(root.join(path))
        .map_err(|_| format!("the skill has no file \"{relative}\""))?;
    if !target.starts_with(&root) {
        return Err(format!("\"{relative}\" is outside the skill"));
    }
    if !target.is_file() {
        return Err(format!("\"{relative}\" is not a file"));
    }
    Ok(target)
}

/// The skill's files besides `SKILL.md`, as `/`-separated relative paths.
pub fn resource_files(dir: &Path) -> Vec<String> {
    let mut files: Vec<String> = walkdir::WalkDir::new(dir)
        .max_depth(MAX_LIST_DEPTH)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| e.file_name() != ".git")
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| {
            let rel = e.path().strip_prefix(dir).ok()?;
            let rel = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            (rel != SKILL_FILE).then_some(rel)
        })
        .collect();
    files.sort();
    files.truncate(MAX_LISTED_FILES);
    files
}

fn find<'a>(skills: &'a [Skill], name: &str) -> Result<&'a Skill, ToolError> {
    skills.iter().find(|s| s.name == name).ok_or_else(|| {
        let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        ToolError::NotFound(format!(
            "No skill named \"{name}\". Available: {}",
            names.join(", ")
        ))
    })
}

/// `load_skill`: a skill's instructions and the list of its files.
pub struct LoadSkillTool {
    skills: Vec<Skill>,
    description: String,
}

impl LoadSkillTool {
    pub fn new(skills: Vec<Skill>) -> Self {
        let mut description = String::from(
            "Load the full instructions of one of the user's skills before doing a task it \
             covers, then follow them. Skills (name: when to use it):",
        );
        for skill in &skills {
            description.push_str(&format!("\n- {}: {}", skill.name, skill.description));
        }
        Self {
            skills,
            description,
        }
    }
}

#[async_trait]
impl HostTool for LoadSkillTool {
    fn name(&self) -> &str {
        LOAD_SKILL
    }
    fn label(&self) -> &str {
        "Load skill"
    }
    fn label_template(&self) -> &str {
        "Loading the {name} skill"
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn schema(&self) -> Value {
        let names: Vec<&str> = self.skills.iter().map(|s| s.name.as_str()).collect();
        json!({
            "type": "object",
            "properties": { "name": { "type": "string", "enum": names } },
            "required": ["name"],
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Read
    }
    fn load_mode(&self) -> ToolLoadMode {
        ToolLoadMode::Essential
    }
    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let name = req_str(&args, "name", LOAD_SKILL)?;
        let skill = find(&self.skills, name)?.clone();
        let (doc, files) = tokio::task::spawn_blocking(move || {
            read_skill(&skill.dir).map(|(_, body)| (body, resource_files(&skill.dir)))
        })
        .await
        .map_err(|e| ToolError::Failed(e.to_string()))?
        .map_err(|e| ToolError::Unavailable(format!("The skill {name} could not be read: {e}")))?;
        let mut text = format!("Skill \"{name}\" (installed by the user):\n\n{doc}");
        if !files.is_empty() {
            text.push_str(&format!(
                "\n\nFiles of this skill (read one with {READ_SKILL_FILE}; never run them):\n"
            ));
            for file in &files {
                text.push_str(&format!("- {file}\n"));
            }
        }
        Ok(ToolOutput {
            text_for_model: text,
            summary_for_ui: format!("Loaded {name}"),
            detail: Some(json!({ "skill": name, "files": files.len() })),
        })
    }
}

/// `read_skill_file`: one file of a skill, never outside its folder.
pub struct ReadSkillFileTool {
    skills: Vec<Skill>,
}

impl ReadSkillFileTool {
    pub fn new(skills: Vec<Skill>) -> Self {
        Self { skills }
    }
}

#[async_trait]
impl HostTool for ReadSkillFileTool {
    fn name(&self) -> &str {
        READ_SKILL_FILE
    }
    fn label(&self) -> &str {
        "Read skill file"
    }
    fn label_template(&self) -> &str {
        "Reading {path} of the {skill} skill"
    }
    fn description(&self) -> &str {
        "Read one file that a skill lists (after load_skill), by its path relative to the \
         skill's folder. Files are only read, never run."
    }
    fn schema(&self) -> Value {
        let names: Vec<&str> = self.skills.iter().map(|s| s.name.as_str()).collect();
        json!({
            "type": "object",
            "properties": {
                "skill": { "type": "string", "enum": names },
                "path": { "type": "string", "minLength": 1, "maxLength": 300 }
            },
            "required": ["skill", "path"],
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Read
    }
    fn load_mode(&self) -> ToolLoadMode {
        ToolLoadMode::Essential
    }
    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let name = req_str(&args, "skill", READ_SKILL_FILE)?.to_string();
        let path = req_str(&args, "path", READ_SKILL_FILE)?.to_string();
        let dir = find(&self.skills, &name)?.dir.clone();
        let wanted = path.clone();
        let text = tokio::task::spawn_blocking(move || {
            confined_path(&dir, &wanted).and_then(|file| read_text(&file))
        })
        .await
        .map_err(|e| ToolError::Failed(e.to_string()))?
        .map_err(|e| ToolError::Forbidden(format!("{path}: {e}")))?;
        Ok(ToolOutput {
            summary_for_ui: format!(
                "{} lines of {}",
                text.lines().count(),
                truncate_chars(&path, 80)
            ),
            text_for_model: text,
            detail: Some(json!({ "skill": name, "path": path })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SKILL: &str = "---\nname: pdf-tools\ndescription: >\n  Extract text and tables\n  from PDF files.\nlicense: MIT\nmetadata:\n  author: someone\n---\n\n# PDF tools\n\nUse pdfplumber.\n";

    #[test]
    fn skill_md_front_matter_is_read() {
        let doc = parse_skill_md(SKILL).unwrap();
        assert_eq!(doc.name, "pdf-tools");
        assert_eq!(doc.description, "Extract text and tables from PDF files.");
        assert_eq!(doc.body, "# PDF tools\n\nUse pdfplumber.");
        let quoted =
            parse_skill_md("---\nname: \"x-y\"\ndescription: 'It''s: useful' # note\n---\nBody")
                .unwrap();
        assert_eq!(quoted.description, "It's: useful");
        let plain_multiline =
            parse_skill_md("---\nname: a\ndescription: first line\n  second line\n---\n").unwrap();
        assert_eq!(plain_multiline.description, "first line second line");
        let literal = parse_skill_md("---\ndescription: |\n  one\n  two\nname: b\n---\n").unwrap();
        assert_eq!(literal.description, "one two");
        assert_eq!(literal.name, "b");
    }

    #[test]
    fn malformed_skills_are_refused() {
        for (text, error) in [
            ("# no front matter", "front matter"),
            ("---\nname: a\ndescription: b\n", "not closed"),
            ("---\ndescription: b\n---\n", "no name"),
            ("---\nname: a\n---\n", "no description"),
            (
                "---\nname: Bad_Name\ndescription: b\n---\n",
                "valid skill name",
            ),
        ] {
            let message = parse_skill_md(text).unwrap_err();
            assert!(message.contains(error), "{text:?}: {message}");
        }
        let long = format!("---\nname: a\ndescription: {}\n---\n", "x".repeat(1_025));
        assert!(parse_skill_md(&long).is_err());
        assert!(is_valid_name("data-analysis-2"));
        assert!(!is_valid_name("-a") && !is_valid_name("a--b") && !is_valid_name(""));
    }

    fn skill_dir(root: &Path, name: &str) -> PathBuf {
        let dir = root.join(name);
        std::fs::create_dir_all(dir.join("scripts")).unwrap();
        std::fs::write(
            dir.join(SKILL_FILE),
            format!("---\nname: {name}\ndescription: Does {name}.\n---\nSteps for {name}."),
        )
        .unwrap();
        std::fs::write(dir.join("scripts").join("run.py"), "print('hi')").unwrap();
        std::fs::write(dir.join("reference.md"), "Reference.").unwrap();
        dir
    }

    #[test]
    fn skills_are_found_in_nested_folders() {
        let root = tempfile::tempdir().unwrap();
        skill_dir(&root.path().join("skills"), "beta");
        skill_dir(root.path(), "alpha");
        std::fs::create_dir_all(root.path().join("broken")).unwrap();
        std::fs::write(root.path().join("broken").join(SKILL_FILE), "nope").unwrap();
        let (skills, problems) = find_skills(root.path(), 3);
        let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["alpha", "beta"]);
        assert_eq!(problems.len(), 1);
        assert_eq!(
            resource_files(&root.path().join("alpha")),
            ["reference.md", "scripts/run.py"]
        );
    }

    #[test]
    fn skill_files_are_confined_to_the_skill_folder() {
        let root = tempfile::tempdir().unwrap();
        let dir = skill_dir(root.path(), "alpha");
        std::fs::write(root.path().join("secret.txt"), "secret").unwrap();
        assert!(confined_path(&dir, "reference.md").is_ok());
        assert!(confined_path(&dir, "./scripts/run.py").is_ok());
        for bad in [
            "../secret.txt",
            "scripts/../../secret.txt",
            "/etc/passwd",
            "\\Windows\\win.ini",
            "C:/Windows/win.ini",
            "reference.md:stream",
            "\\\\server\\share\\x",
            "",
            "scripts",
            "missing.md",
        ] {
            assert!(confined_path(&dir, bad).is_err(), "{bad:?} was allowed");
        }
        let absolute = root.path().join("secret.txt").display().to_string();
        assert!(confined_path(&dir, &absolute).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn links_out_of_the_skill_are_refused() {
        let root = tempfile::tempdir().unwrap();
        let dir = skill_dir(root.path(), "alpha");
        std::fs::write(root.path().join("secret.txt"), "secret").unwrap();
        std::os::unix::fs::symlink(root.path().join("secret.txt"), dir.join("link.txt")).unwrap();
        assert!(confined_path(&dir, "link.txt").is_err());
    }

    #[tokio::test]
    async fn the_tools_load_instructions_and_read_files() {
        let root = tempfile::tempdir().unwrap();
        skill_dir(root.path(), "alpha");
        let (skills, _) = find_skills(root.path(), 1);
        let load = LoadSkillTool::new(skills.clone());
        assert!(load.description().contains("\n- alpha: Does alpha."));
        assert_eq!(
            load.schema()["properties"]["name"]["enum"],
            json!(["alpha"])
        );
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let ctx = ToolContext::new("r", "s", tx);
        let out = load.execute(json!({"name": "alpha"}), &ctx).await.unwrap();
        assert!(out.text_for_model.contains("Steps for alpha."));
        assert!(out.text_for_model.contains("- scripts/run.py"));
        assert!(load.execute(json!({"name": "nope"}), &ctx).await.is_err());
        let read = ReadSkillFileTool::new(skills);
        let out = read
            .execute(json!({"skill": "alpha", "path": "reference.md"}), &ctx)
            .await
            .unwrap();
        assert_eq!(out.text_for_model, "Reference.");
        let refused = read
            .execute(json!({"skill": "alpha", "path": "../alpha/../x"}), &ctx)
            .await;
        assert!(matches!(refused, Err(ToolError::Forbidden(_))));
    }
}
