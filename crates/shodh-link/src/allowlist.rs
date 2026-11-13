//! What a phone may ask the desktop to do. Checked on the desktop: anything
//! not listed is refused, and an approval the phone sends is granted only for
//! the kinds a remote device may grant ([`remote_may_approve`]).

use std::path::{Component, Path, PathBuf};

/// Whether an entry is one of the desktop's Tauri commands, forwarded as the
/// desktop window would call it, or an operation only the bridge has
/// (because no command fits the phone, mostly the 4 MiB frame cap).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    /// A Tauri command the desktop registers (`generate_handler!`).
    Command,
    /// Served by the desktop's bridge only; the desktop window has no such
    /// command.
    BridgeOnly,
}

/// One thing the phone may call, and why it may.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub command: &'static str,
    pub kind: EntryKind,
    pub why: &'static str,
}

const fn command(command: &'static str, why: &'static str) -> Entry {
    Entry {
        command,
        kind: EntryKind::Command,
        why,
    }
}

const fn bridge_only(command: &'static str, why: &'static str) -> Entry {
    Entry {
        command,
        kind: EntryKind::BridgeOnly,
        why,
    }
}

/// Everything a phone may call: what the phone's screens (Ask,
/// Conversations, Library, Reader, Approvals) need, and nothing else. No
/// setting, no source change, no deletion, no Code mode.
pub const ALLOWED: &[Entry] = &[
    // Ask
    command(
        "agent_start",
        "Ask: starts (or reuses) the conversation's agent session before a question. The \
         desktop refuses Code mode and the modes that skip approvals (Accept edits, Auto) \
         from a phone.",
    ),
    command(
        "agent_send",
        "Ask: sends the question. Refused into a Code session; the session's approval \
         gate decides every step exactly as for a question typed on the desktop.",
    ),
    command(
        "agent_abort",
        "Ask: the Stop button. It only ends the running answer.",
    ),
    command(
        "agent_approve",
        "Approvals: Allow or Deny on an approval card the phone was sent. Only Ask and \
         Creator steps may be allowed from a phone (remote_may_approve); the rest wait for \
         the computer.",
    ),
    command(
        "workspaces_list",
        "Ask and Library: the workspace picker (names and sources of the user's \
         workspaces). Read only.",
    ),
    // Conversations
    bridge_only(
        "conversations_list",
        "Conversations: the list (title, time, workspace, pin) from the desktop's \
         load_conversations, without the messages, which as a whole exceed one frame.",
    ),
    bridge_only(
        "conversation_load",
        "Conversations: one conversation with its messages, to read and continue it. Read \
         only; the desktop saves answers itself.",
    ),
    // Library and Reader
    bridge_only(
        "library_folders",
        "Library: the user's linked folders, then the child folders and indexed files of \
         one folder inside them, at any depth. A path outside the linked folders is \
         refused.",
    ),
    command(
        "get_source_file_info",
        "Reader: how to show an indexed file (its kind, size and name). Only files the index \
         holds, inside the linked folders.",
    ),
    command(
        "get_pdf_info",
        "Reader: a PDF's page count and page size. Only indexed files inside the linked \
         folders.",
    ),
    bridge_only(
        "read_source_page",
        "Reader: one PDF page (or an image) rendered to PNG at the phone's width. The whole \
         file (read_source_bytes) can exceed one frame; a page fits.",
    ),
    command(
        "read_source_text",
        "Reader: the extracted text of a text, Markdown, code or Office file, sent in parts \
         that each fit one frame. Only indexed files inside the linked folders.",
    ),
    command(
        "read_source_table",
        "Reader: the sheets of a spreadsheet or CSV file. Only indexed files inside the \
         linked folders.",
    ),
];

/// Whether a phone may call `command`.
pub fn allowed(command: &str) -> bool {
    ALLOWED.iter().any(|e| e.command == command)
}

/// The argument of each allowlisted command that names a file or folder on
/// the desktop. The bridge checks it is inside the user's linked folders
/// before the command runs.
pub const PATH_ARGS: &[(&str, &str)] = &[
    ("library_folders", "path"),
    ("get_source_file_info", "filePath"),
    ("get_pdf_info", "filePath"),
    ("read_source_page", "filePath"),
    ("read_source_text", "filePath"),
    ("read_source_table", "filePath"),
];

/// The path argument of `command`, if it has one.
pub fn path_arg(command: &str) -> Option<&'static str> {
    PATH_ARGS
        .iter()
        .find(|(c, _)| *c == command)
        .map(|(_, arg)| *arg)
}

/// Why a path from the phone was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PathRefusal {
    #[error(
        "Only files and folders in the folders you linked to Shodh can be opened from your phone."
    )]
    OutsideLinkedFolders,
    #[error("This is not a file on your computer.")]
    NotAFile,
}

/// The shape every path from the phone must have before anything looks at
/// the disk: absolute on the desktop, without `.` or `..` components, not a
/// URL, no NUL.
pub fn path_shape(raw: &str) -> Result<PathBuf, PathRefusal> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.contains('\0') || trimmed.contains("://") {
        return Err(PathRefusal::NotAFile);
    }
    let path = Path::new(trimmed);
    // `components()` drops a `.` inside a path, so the text is checked too
    // (both separators: a phone's path is the desktop's spelling).
    let dotted = trimmed.split(['/', '\\']).any(|s| s == "." || s == "..");
    if dotted
        || !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err(PathRefusal::OutsideLinkedFolders);
    }
    Ok(path.to_path_buf())
}

/// What an approval the phone answers would allow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalKind {
    /// A step of an Ask (research) answer: a web fetch, a note, an export, a
    /// GitHub post.
    AskStep,
    /// A step of a Creator answer, inside its creation folder.
    CreatorStep,
    /// A command in Code mode.
    CodeCommand,
    /// A file edit or write.
    FileEdit,
    /// Any other step of a Code session.
    CodeStep,
    /// "Allow for the rest of this conversation".
    AllowForConversation,
    /// A step the phone was never sent, or one the desktop cannot place.
    Unknown,
}

/// What the phone shows instead of Allow for an approval it may not grant.
pub const APPROVE_ON_COMPUTER: &str = "Approve this on your computer";

/// Whether a phone may grant an approval of `kind`: Ask and Creator steps
/// only. Code-mode commands, file edits, every Code step, "allow for the
/// rest of this conversation" and anything unknown wait for the computer.
pub fn remote_may_approve(kind: &ApprovalKind) -> bool {
    matches!(kind, ApprovalKind::AskStep | ApprovalKind::CreatorStep)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_outside_the_allowlist_is_refused() {
        for refused in [
            "save_conversation",
            "delete_conversation",
            "clear_all_documents",
            "reset_database",
            "set_app_policy",
            "set_default_agent_mode",
            "code_settings_set",
            "read_source_bytes",
            "jump_to_source",
            "agent_steer",
            "run_command_stop",
            "phone_device_remove",
            "phone_pairing_start",
            "telegram_link_code",
            "",
            "agent_start ",
            "AGENT_START",
        ] {
            assert!(!allowed(refused), "{refused} must be refused");
        }
        for entry in ALLOWED {
            assert!(allowed(entry.command));
        }
    }

    #[test]
    fn every_entry_says_why_and_is_listed_once() {
        for (i, entry) in ALLOWED.iter().enumerate() {
            assert!(entry.why.len() >= 40, "{}: say why", entry.command);
            assert!(
                !ALLOWED[i + 1..].iter().any(|e| e.command == entry.command),
                "{} listed twice",
                entry.command
            );
        }
        for (command, _) in PATH_ARGS {
            assert!(allowed(command), "{command} has a path but is not allowed");
        }
    }

    #[test]
    fn only_ask_and_creator_steps_may_be_approved_remotely() {
        assert!(remote_may_approve(&ApprovalKind::AskStep));
        assert!(remote_may_approve(&ApprovalKind::CreatorStep));
        for kind in [
            ApprovalKind::CodeCommand,
            ApprovalKind::FileEdit,
            ApprovalKind::CodeStep,
            ApprovalKind::AllowForConversation,
            ApprovalKind::Unknown,
        ] {
            assert!(!remote_may_approve(&kind), "{kind:?}");
        }
    }

    #[test]
    fn a_path_must_be_absolute_without_dots() {
        let root = std::env::temp_dir();
        let inside = root.join("papers").join("self-rag.pdf");
        assert_eq!(
            path_shape(&inside.display().to_string()),
            Ok(inside.clone())
        );
        let up = format!(
            "{}{}..{}secret.txt",
            root.display(),
            std::path::MAIN_SEPARATOR,
            std::path::MAIN_SEPARATOR
        );
        assert_eq!(path_shape(&up), Err(PathRefusal::OutsideLinkedFolders));
        let here = format!(
            "{}{}.{}a.txt",
            root.display(),
            std::path::MAIN_SEPARATOR,
            std::path::MAIN_SEPARATOR
        );
        assert_eq!(path_shape(&here), Err(PathRefusal::OutsideLinkedFolders));
        assert_eq!(
            path_shape("papers/self-rag.pdf"),
            Err(PathRefusal::OutsideLinkedFolders)
        );
        assert_eq!(
            path_shape("../etc/passwd"),
            Err(PathRefusal::OutsideLinkedFolders)
        );
        assert_eq!(
            path_shape("https://example.org/a.pdf"),
            Err(PathRefusal::NotAFile)
        );
        assert_eq!(path_shape("note://abc"), Err(PathRefusal::NotAFile));
        assert_eq!(path_shape("   "), Err(PathRefusal::NotAFile));
        assert_eq!(path_shape("C:/a\0b"), Err(PathRefusal::NotAFile));
    }
}
