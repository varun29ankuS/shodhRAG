//! System integration module - Cross-platform file operations and OS integration
//!
//! Provides abstraction over OS-specific APIs for:
//! - File/folder creation
//! - Command execution
//! - System queries
//! - Native integrations

pub mod file_ops;
pub mod command_executor;
pub mod os_integration;

pub use file_ops::{
    FileSystemAction, FileSystemResult, FolderStructure,
    create_folder_structure, create_file, copy_path, move_path, delete_path, list_directory
};

pub use command_executor::{
    CommandAction, CommandResult, CommandRiskLevel,
    execute_command, execute_powershell, execute_bash, analyze_command_risk
};

pub use os_integration::{
    open_in_file_manager, get_system_info, list_running_processes
};
