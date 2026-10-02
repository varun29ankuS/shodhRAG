//! System integration module - Cross-platform file operations and OS integration
//!
//! Provides abstraction over OS-specific APIs for:
//! - File/folder creation
//! - Command execution
//! - System queries
//! - Native integrations

pub mod command_executor;
pub mod file_ops;
pub mod os_integration;

pub use file_ops::{
    copy_path, create_file, create_folder_structure, delete_path, list_directory, move_path,
    FileSystemAction, FileSystemResult, FolderStructure,
};

pub use command_executor::{
    analyze_command_risk, execute_bash, execute_command, execute_powershell, CommandAction,
    CommandResult, CommandRiskLevel,
};

pub use os_integration::{get_system_info, list_running_processes, open_in_file_manager};
