//! Cross-platform file operations
//! Abstraction over std::fs with enhanced error handling and cross-platform path support

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// File system action (can be serialized from LLM JSON)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FileSystemAction {
    CreateFolders {
        base_path: PathBuf,
        structure: FolderStructure,
    },
    CreateFile {
        path: PathBuf,
        content: String,
        #[serde(default)]
        overwrite: bool,
    },
    Copy {
        source: PathBuf,
        destination: PathBuf,
    },
    Move {
        source: PathBuf,
        destination: PathBuf,
    },
    Delete {
        path: PathBuf,
        #[serde(default)]
        recursive: bool,
    },
    ListDirectory {
        path: PathBuf,
        #[serde(default)]
        recursive: bool,
    },
}

/// Folder structure specification (recursive)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FolderStructure {
    /// Simple list of folder names
    Simple(Vec<String>),
    /// Nested structure with subfolders
    Nested(HashMap<String, FolderStructure>),
}

/// Result of file system operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileSystemResult {
    pub success: bool,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub affected_paths: Option<Vec<String>>,
}

/// Create folder structure recursively
pub fn create_folder_structure(
    base: &Path,
    structure: &FolderStructure,
) -> Result<FileSystemResult> {
    let mut created_paths = Vec::new();

    match structure {
        FolderStructure::Simple(folders) => {
            for folder in folders {
                let path = base.join(folder);
                fs::create_dir_all(&path)
                    .context(format!("Failed to create folder: {:?}", path))?;
                created_paths.push(path.to_string_lossy().to_string());
            }
        }

        FolderStructure::Nested(map) => {
            for (folder, sub_structure) in map {
                let path = base.join(folder);
                fs::create_dir_all(&path)
                    .context(format!("Failed to create folder: {:?}", path))?;
                created_paths.push(path.to_string_lossy().to_string());

                // Recursively create subfolders
                let sub_result = create_folder_structure(&path, sub_structure)?;
                if let Some(mut sub_paths) = sub_result.affected_paths {
                    created_paths.append(&mut sub_paths);
                }
            }
        }
    }

    Ok(FileSystemResult {
        success: true,
        message: format!("Created {} folders", created_paths.len()),
        output: None,
        affected_paths: Some(created_paths),
    })
}

/// Create a file with content
pub fn create_file(path: &Path, content: &str, overwrite: bool) -> Result<FileSystemResult> {
    // Check if file exists
    if path.exists() && !overwrite {
        return Err(anyhow!(
            "File already exists: {:?}. Set overwrite=true to replace.",
            path
        ));
    }

    // Create parent directories if they don't exist
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).context("Failed to create parent directories")?;
    }

    // Write file
    fs::write(path, content).context(format!("Failed to write file: {:?}", path))?;

    Ok(FileSystemResult {
        success: true,
        message: format!("Created file: {:?}", path),
        output: None,
        affected_paths: Some(vec![path.to_string_lossy().to_string()]),
    })
}

/// Copy file or folder (cross-platform)
pub fn copy_path(source: &Path, destination: &Path) -> Result<FileSystemResult> {
    if !source.exists() {
        return Err(anyhow!("Source does not exist: {:?}", source));
    }

    if source.is_file() {
        // Create parent directory if needed
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(source, destination).context("Failed to copy file")?;
    } else {
        // Copy directory recursively
        copy_dir_all(source, destination)?;
    }

    Ok(FileSystemResult {
        success: true,
        message: format!("Copied {:?} to {:?}", source, destination),
        output: None,
        affected_paths: Some(vec![destination.to_string_lossy().to_string()]),
    })
}

/// Copy directory recursively
fn copy_dir_all(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;

    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if src_path.is_dir() {
            copy_dir_all(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }

    Ok(())
}

/// Move/rename file or folder
pub fn move_path(source: &Path, destination: &Path) -> Result<FileSystemResult> {
    if !source.exists() {
        return Err(anyhow!("Source does not exist: {:?}", source));
    }

    // Create parent directory if needed
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }

    fs::rename(source, destination).context("Failed to move/rename")?;

    Ok(FileSystemResult {
        success: true,
        message: format!("Moved {:?} to {:?}", source, destination),
        output: None,
        affected_paths: Some(vec![destination.to_string_lossy().to_string()]),
    })
}

/// Delete file or folder
pub fn delete_path(path: &Path, recursive: bool) -> Result<FileSystemResult> {
    if !path.exists() {
        return Err(anyhow!("Path does not exist: {:?}", path));
    }

    if path.is_file() {
        fs::remove_file(path).context("Failed to delete file")?;
    } else {
        if recursive {
            fs::remove_dir_all(path).context("Failed to delete directory")?;
        } else {
            fs::remove_dir(path)
                .context("Failed to delete directory (not empty, use recursive=true)")?;
        }
    }

    Ok(FileSystemResult {
        success: true,
        message: format!("Deleted: {:?}", path),
        output: None,
        affected_paths: Some(vec![path.to_string_lossy().to_string()]),
    })
}

/// List directory contents
pub fn list_directory(path: &Path, recursive: bool) -> Result<FileSystemResult> {
    if !path.exists() {
        return Err(anyhow!("Path does not exist: {:?}", path));
    }

    if !path.is_dir() {
        return Err(anyhow!("Path is not a directory: {:?}", path));
    }

    let mut files = Vec::new();

    if recursive {
        list_dir_recursive(path, &mut files)?;
    } else {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let path = entry.path();
            let file_type = if path.is_dir() { "DIR" } else { "FILE" };
            let size = if path.is_file() {
                fs::metadata(&path)
                    .map(|m| format!(" ({}B)", m.len()))
                    .unwrap_or_default()
            } else {
                String::new()
            };
            files.push(format!(
                "[{}] {}{}",
                file_type,
                path.to_string_lossy(),
                size
            ));
        }
    }

    let output = files.join("\n");

    Ok(FileSystemResult {
        success: true,
        message: format!("Listed {} items", files.len()),
        output: Some(output),
        affected_paths: None,
    })
}

/// Recursively list directory
fn list_dir_recursive(path: &Path, files: &mut Vec<String>) -> Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let entry_path = entry.path();
        let file_type = if entry_path.is_dir() { "DIR" } else { "FILE" };
        let size = if entry_path.is_file() {
            fs::metadata(&entry_path)
                .map(|m| format!(" ({}B)", m.len()))
                .unwrap_or_default()
        } else {
            String::new()
        };
        files.push(format!(
            "[{}] {}{}",
            file_type,
            entry_path.to_string_lossy(),
            size
        ));

        if entry_path.is_dir() {
            list_dir_recursive(&entry_path, files)?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn test_create_simple_folders() {
        let temp_dir = env::temp_dir().join("shodh_test_folders");

        let structure = FolderStructure::Simple(vec!["folder1".to_string(), "folder2".to_string()]);

        let result = create_folder_structure(&temp_dir, &structure).unwrap();
        assert!(result.success);
        assert_eq!(result.affected_paths.as_ref().unwrap().len(), 2);

        // Cleanup
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn test_create_file() {
        let temp_dir = env::temp_dir();
        let file_path = temp_dir.join("shodh_test_file.txt");

        let result = create_file(&file_path, "Hello, Shodh!", false).unwrap();
        assert!(result.success);
        assert!(file_path.exists());

        let content = fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "Hello, Shodh!");

        // Cleanup
        let _ = fs::remove_file(file_path);
    }

    #[test]
    fn test_list_directory() {
        let temp_dir = env::temp_dir();
        let result = list_directory(&temp_dir, false).unwrap();
        assert!(result.success);
        assert!(result.output.is_some());
    }
}
