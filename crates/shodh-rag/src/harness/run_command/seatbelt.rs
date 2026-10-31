// Portions of this file are derived from OpenAI Codex
// (https://github.com/openai/codex, commit
// 67f0994b532b8740f14b5b738af7b7b2a915b1d6, Copyright 2025 OpenAI) and
// Anthropic's sandbox-runtime
// (https://github.com/anthropic-experimental/sandbox-runtime, commit
// 98a4016d27609ed34dd1fb43d23ed6efb6eeeb77, Copyright 2025 Anthropic), both
// licensed under the Apache License, Version 2.0
// (http://www.apache.org/licenses/LICENSE-2.0). Modified by Shodh; see NOTICE.

//! macOS: the Seatbelt profile one command runs under, rendered from its
//! [`SandboxPolicy`], and how `/usr/bin/sandbox-exec` is checked and run.
//!
//! The profile denies everything by default, then:
//! - the base rules ([`BASE_POLICY`], OpenAI Codex's base policy, vendored):
//!   starting processes, signals inside the sandbox, the system values and
//!   services ordinary programs ask for;
//! - reads: everything the system lets any account read, except the user's
//!   home; inside it, only the readable folders of the policy; never the
//!   policy's deny list (credentials, keychains, Shodh's data); the code
//!   folder and the scratch folder always;
//! - the metadata (not the contents) of the folders above the readable and
//!   writable ones, so `realpath` and `getcwd` work in a project inside the
//!   home (as in Anthropic's sandbox-runtime);
//! - writes: only the code folder and the scratch folder, which themselves
//!   cannot be moved or removed;
//! - a deny-list folder inside the code folder (a project such as
//!   `~/.config` holds `~/.config/gh`) is closed again after the writable
//!   folders are opened, for reads and changes, and the folders between it
//!   and the code folder cannot be moved, so it cannot be carried out from
//!   under its rule;
//! - no keychain: the security services are denied by name, after every
//!   allow;
//! - no network, in either direction, loopback included.
//!
//! Seatbelt applies the last rule that matches, so the order above is the
//! order of the profile. Paths reach the profile as parameters
//! (`-DKEY=path`, read with `(param "KEY")`, as Codex does), never inside
//! its text, so no folder name can change what the profile says.
//!
//! Based on the Apache-2.0 policies of OpenAI Codex
//! (`codex-rs/sandboxing/src/seatbelt.rs`, `seatbelt_base_policy.sbpl`,
//! `seatbelt_network_policy.sbpl`) and Anthropic's sandbox-runtime
//! (`src/sandbox/macos-sandbox-utils.ts`), at the commits named in NOTICE.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::policy::{within, SandboxPolicy};

/// The one `sandbox-exec` Shodh runs: never one found on `PATH`, which any
/// program of the user can change.
pub const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// Codex's base policy, vendored with its notice.
const BASE_POLICY: &str = include_str!("seatbelt_base_policy.sbpl");

/// What a profile is checked with before the command runs: a program that
/// only exits.
const CHECK_PROGRAM: &str = "/usr/bin/true";

/// The security services that hold the keychains' secrets.
const KEYCHAIN_SERVICES: &[&str] = &[
    "com.apple.SecurityServer",
    "com.apple.securityd",
    "com.apple.securityd.xpc",
    "com.apple.secd",
    "com.apple.security.agent",
    "com.apple.security.authhost",
];

/// A rendered profile: its text and the paths its parameters name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub text: String,
    pub params: Vec<(String, PathBuf)>,
}

/// The parameters of a profile being rendered.
#[derive(Default)]
struct Params(Vec<(String, PathBuf)>);

impl Params {
    /// `path` as the parameter `<prefix>_<n>`; its filter is returned.
    fn add(&mut self, prefix: &str, path: &Path) -> String {
        let key = format!("{prefix}_{}", self.0.len());
        self.0.push((key.clone(), path.to_path_buf()));
        key
    }

    /// `(subpath (param ..))` filters of `paths`.
    fn subpaths(&mut self, prefix: &str, paths: &[PathBuf]) -> String {
        paths
            .iter()
            .map(|path| format!(" (subpath (param \"{}\"))", self.add(prefix, path)))
            .collect()
    }
}

/// The folders above the readable and writable ones that the private home
/// or the deny list would otherwise hide: their metadata is readable.
fn ancestors(policy: &SandboxPolicy) -> Vec<PathBuf> {
    let hidden = |dir: &Path| {
        policy
            .private
            .as_deref()
            .is_some_and(|home| within(dir, home))
            || policy.deny_read.iter().any(|deny| within(dir, deny))
    };
    let mut found: Vec<PathBuf> = Vec::new();
    for root in policy.writable.iter().chain(&policy.readable) {
        for dir in root.ancestors().skip(1) {
            if hidden(dir) && !found.iter().any(|known| known == dir) {
                found.push(dir.to_path_buf());
            }
        }
    }
    found
}

/// The Seatbelt profile of `policy`.
pub fn render(policy: &SandboxPolicy) -> Profile {
    let mut params = Params::default();
    let mut text = String::from(BASE_POLICY);
    text.push_str("\n; Shodh: files\n");
    text.push_str("; Everything the system lets any account read...\n(allow file-read*)\n");
    if let Some(home) = &policy.private {
        let key = params.add("PRIVATE", home);
        text.push_str(&format!(
            "; ...except the user's home...\n(deny file-read* (subpath (param \"{key}\")))\n"
        ));
    }
    if !policy.readable.is_empty() {
        let filters = params.subpaths("READABLE", &policy.readable);
        text.push_str(&format!(
            "; ...where only these folders are readable...\n(allow file-read*{filters})\n"
        ));
    }
    if !policy.deny_read.is_empty() {
        let filters = params.subpaths("DENY", &policy.deny_read);
        text.push_str(&format!(
            "; ...and never these, wherever they are.\n(deny file-read*{filters})\n"
        ));
    }
    let writable = params.subpaths("WRITABLE", &policy.writable);
    text.push_str(&format!(
        "; The code folder and the scratch folder: read and changed.\n\
         (allow file-read* file-write*{writable})\n"
    ));
    let inside = policy.deny_inside_writable();
    if !inside.is_empty() {
        let owned: Vec<PathBuf> = inside.iter().map(|deny| deny.to_path_buf()).collect();
        let filters = params.subpaths("INSIDE", &owned);
        text.push_str(&format!(
            "; Deny-list folders inside them stay closed.\n(deny file-read* file-write*{filters})\n"
        ));
        let mut between: Vec<PathBuf> = Vec::new();
        for deny in &owned {
            let Some(root) = policy.writable.iter().find(|root| within(deny, root)) else {
                continue;
            };
            for dir in deny.ancestors().skip(1) {
                if dir == root.as_path() || !within(dir, root) {
                    break;
                }
                if !between.iter().any(|known| known == dir) {
                    between.push(dir.to_path_buf());
                }
            }
        }
        for dir in between {
            let key = params.add("BETWEEN", &dir);
            text.push_str(&format!(
                "(deny file-write-unlink (require-all (vnode-type DIRECTORY) (literal (param \"{key}\"))))\n"
            ));
        }
    }
    let above = ancestors(policy);
    if !above.is_empty() {
        let filters: String = above
            .iter()
            .map(|dir| format!(" (literal (param \"{}\"))", params.add("ABOVE", dir)))
            .collect();
        text.push_str(&format!(
            "; The folders above them: metadata only, for realpath and getcwd.\n\
             (allow file-read-metadata{filters})\n"
        ));
    }
    for root in &policy.writable {
        let key = params.add("ROOT", root);
        text.push_str(&format!(
            "(deny file-write-unlink (require-all (vnode-type DIRECTORY) (literal (param \"{key}\"))))\n"
        ));
    }
    text.push_str(
        "; Fcntls that change a file through a descriptor opened only to read it\n\
         ; (F_MAKECOMPRESSED, F_TRANSFEREXTENTS), as in Codex.\n\
         (deny system-fcntl (fcntl-command 80 110))\n",
    );
    text.push_str(
        "; The command's own output, and the device files programs ask about.\n\
         (allow file-write-data (literal \"/dev/stdout\") (literal \"/dev/stderr\") \
         (regex #\"^/dev/fd/[0-9]+$\"))\n\
         (allow file-ioctl (literal \"/dev/null\") (literal \"/dev/zero\") \
         (literal \"/dev/random\") (literal \"/dev/urandom\") (literal \"/dev/tty\"))\n",
    );
    text.push_str("\n; Shodh: no keychain\n(deny mach-lookup");
    for service in KEYCHAIN_SERVICES {
        text.push_str(&format!(" (global-name \"{service}\")"));
    }
    text.push_str(")\n(deny mach-lookup (xpc-service-name-prefix \"\"))\n");
    text.push_str("\n; Shodh: network\n");
    if policy.network {
        text.push_str("(allow network*)\n");
    } else {
        text.push_str("(deny network*)\n");
    }
    Profile {
        text,
        params: params.0,
    }
}

/// `sandbox-exec`'s arguments for `profile`, up to the program it runs.
pub fn arguments(profile: &Profile) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec!["-p".into(), profile.text.clone().into()];
    for (key, path) in &profile.params {
        let mut define = OsString::from(format!("-D{key}="));
        define.push(path.as_os_str());
        args.push(define);
    }
    args.push("--".into());
    args
}

/// Refuse unless `sandbox_exec` is there and accepts `profile`: it runs
/// [`CHECK_PROGRAM`] under the profile first, and any failure refuses the
/// command, so a command never runs outside the sandbox.
pub fn check(sandbox_exec: &Path, profile: &Profile) -> Result<(), String> {
    if !sandbox_exec.is_file() {
        return Err(format!(
            "Commands run in macOS's sandbox ({}), which was not found on this Mac, so the \
             command was not run.",
            sandbox_exec.display()
        ));
    }
    let checked = Command::new(sandbox_exec)
        .args(arguments(profile))
        .arg(CHECK_PROGRAM)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| {
            format!(
                "Shodh could not start macOS's sandbox ({}: {e}), so the command was not run.",
                sandbox_exec.display()
            )
        })?;
    if checked.status.success() {
        return Ok(());
    }
    let said = String::from_utf8_lossy(&checked.stderr);
    let said = said
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("no reason given");
    Err(format!(
        "macOS's sandbox rejected the command's sandbox profile ({said}), so the command was \
         not run."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> SandboxPolicy {
        SandboxPolicy {
            writable: vec![
                PathBuf::from("/Users/me/.config"),
                PathBuf::from("/Users/me/Library/Shodh/code-scratch/abc"),
            ],
            readable: vec![PathBuf::from("/Users/me/.cargo")],
            private: Some(PathBuf::from("/Users/me")),
            deny_read: vec![
                PathBuf::from("/Users/me/.ssh"),
                PathBuf::from("/Users/me/.config/git/credentials"),
                PathBuf::from("/Users/me/Library/Shodh"),
            ],
            network: false,
        }
    }

    /// The value of the parameter a filter names, by path.
    fn key_of(profile: &Profile, path: &str) -> Vec<String> {
        profile
            .params
            .iter()
            .filter(|(_, value)| value == Path::new(path))
            .map(|(key, _)| key.clone())
            .collect()
    }

    #[test]
    fn the_keychain_services_are_denied_by_name_after_every_allow() {
        // Belt and braces: deny-default already closes them; the explicit
        // rule keeps them closed if an allow above ever opened mach lookups.
        let profile = render(&policy());
        let rule = profile
            .text
            .find("(deny mach-lookup (global-name")
            .expect("the keychain rule");
        for service in KEYCHAIN_SERVICES {
            let named = format!("(global-name \"{service}\")");
            let at = profile.text.rfind(&named).expect(service);
            assert!(at > rule, "{service}");
        }
        assert!(profile.text.contains("\"com.apple.SecurityServer\""));
        assert!(profile.text.contains("\"com.apple.securityd.xpc\""));
        // Nothing after the rule allows a lookup again.
        assert!(!profile.text[rule..].contains("(allow mach-lookup"));
        assert!(profile.text.trim_end().ends_with("(deny network*)"));
    }

    #[test]
    fn a_deny_list_folder_inside_the_project_is_closed_again_after_it_is_opened() {
        let profile = render(&policy());
        let writable = profile
            .text
            .find("(allow file-read* file-write*")
            .expect("the writable rule");
        let inside = key_of(&profile, "/Users/me/.config/git/credentials");
        let inside = inside
            .iter()
            .find(|key| key.starts_with("INSIDE_"))
            .expect("the folder inside the project is closed again");
        let closed = profile
            .text
            .find(&format!(
                "(deny file-read* file-write* (subpath (param \"{inside}\"))"
            ))
            .expect("the closing rule");
        assert!(closed > writable);
        // The folder between it and the project cannot be moved away.
        let between = key_of(&profile, "/Users/me/.config/git");
        assert!(
            between.iter().any(|key| key.starts_with("BETWEEN_")),
            "{between:?}"
        );
        // Folders that contain a writable one (Shodh's data) are not closed
        // again: the scratch folder stays open.
        assert!(key_of(&profile, "/Users/me/Library/Shodh")
            .iter()
            .all(|key| !key.starts_with("INSIDE_")));
    }
}
