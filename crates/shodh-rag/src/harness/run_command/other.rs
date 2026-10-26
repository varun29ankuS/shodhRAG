//! Systems other than Windows and macOS: commands are refused. Shodh's
//! process limits ([`super::win`], [`super::mac`]) exist for those two only,
//! and a command never runs without them.

use std::path::{Path, PathBuf};

use tokio::process::Child;

use super::{CommandSpec, Killer};

/// Why commands do not run here.
const UNSUPPORTED: &str =
    "Commands run only on Windows and macOS, where Shodh can limit and stop them.";

/// No shell: commands are refused on this system.
pub fn locate_shell() -> Result<PathBuf, String> {
    Err(UNSUPPORTED.to_string())
}

/// Make `dir` readable, writable and enterable by its owner only (0700).
pub fn make_private(dir: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("{} could not be made private: {e}", dir.display()))
    }
    #[cfg(not(unix))]
    {
        Err(format!(
            "{} could not be made private: {UNSUPPORTED}",
            dir.display()
        ))
    }
}

/// No limit ends a process by a signal here.
#[cfg(unix)]
pub fn signal_exit(_signal: i32) -> Option<&'static str> {
    None
}

/// Commands are refused on this system.
pub fn spawn_contained(
    _shell: &Path,
    _spec: &CommandSpec,
    _env: &[(String, String)],
    _command_id: &str,
) -> Result<(Child, Contained), String> {
    Err(UNSUPPORTED.to_string())
}

/// A running command: none can exist on this system.
#[derive(Debug)]
pub enum Contained {}

impl Contained {
    pub fn over_limit(&mut self) -> Option<String> {
        match *self {}
    }

    pub fn terminate(&mut self) -> usize {
        match *self {}
    }

    pub fn notes(&mut self) -> Vec<String> {
        match *self {}
    }

    pub fn killer(&self) -> Killer {
        match *self {}
    }
}
