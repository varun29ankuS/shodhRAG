//! Creator mode's package installs.
//!
//! Where commands are sandboxed, a Creator command in its folder runs without
//! asking, with no network. A package install needs the network, and macOS's
//! sandbox cannot limit a command's network to a package registry (Seatbelt
//! filters by address and port, never by host name), so the packages' own
//! install scripts would reach the whole internet. Installs therefore always
//! ask, in every mode and on every system:
//!
//! - [`mentions_install`] is broad: any command that names a package manager
//!   with a subcommand that fetches (or a runner that downloads what it runs,
//!   such as `npx`) asks. Missing one costs nothing: run without asking in the
//!   sandbox, it has no network and fails.
//! - [`install`] is narrow: one plain command (the rules' characters,
//!   [`super::rules::PLAIN_CHARS`]) of a known package manager, with options
//!   that keep it in the folder and package names that are names (no URL, no
//!   path, no other registry). Only such an install, once the user approves
//!   it, runs in the sandbox with the network on
//!   ([`super::super::run_command::CommandSpec::network`]); it still changes
//!   nothing outside the creation folder and the conversation's scratch
//!   folder. Any other approved command keeps the sandbox's network off.

use super::rules::{accept, outside_project, shown, special_reason, Grammar, Positionals, NONE};

/// Longest install command Shodh reads, in characters.
const MAX_INSTALL_CHARS: usize = 300;

/// A package install Shodh recognises: it may get the network once approved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Install {
    /// The package manager, as the prompt names it ("npm").
    pub manager: &'static str,
    /// The packages it fetches run their own install scripts (npm, pnpm and yarn
    /// lifecycle scripts unless `--ignore-scripts`; pip build steps unless
    /// `--only-binary :all:`). cargo fetch and add, and go, run none.
    pub scripts: bool,
}

/// Programs that download what they run, whatever follows.
const DOWNLOAD_RUNNERS: [&str; 7] = ["npx", "pnpx", "bunx", "uvx", "pipx", "corepack", "rustup"];

/// Package managers and the subcommands that make them fetch packages.
const FETCHING: &[(&str, &[&str])] = &[
    (
        "npm",
        &[
            "install",
            "i",
            "in",
            "ins",
            "inst",
            "insta",
            "instal",
            "isnt",
            "isnta",
            "isntal",
            "isntall",
            "add",
            "ci",
            "clean-install",
            "ic",
            "install-clean",
            "isntall-clean",
            "install-test",
            "it",
            "install-ci-test",
            "cit",
            "update",
            "up",
            "upgrade",
            "udpate",
            "exec",
            "x",
            "create",
            "init",
            "innit",
            "dedupe",
            "rebuild",
        ],
    ),
    (
        "pnpm",
        &[
            "install", "i", "add", "update", "up", "upgrade", "dlx", "create", "fetch", "import",
            "dedupe", "rebuild", "env",
        ],
    ),
    (
        "yarn",
        &[
            "install", "add", "upgrade", "up", "dlx", "create", "set", "plugin", "import",
            "dedupe", "rebuild",
        ],
    ),
    (
        "bun",
        &[
            "install", "i", "add", "a", "update", "x", "create", "init", "upgrade", "link",
        ],
    ),
    ("pip", &["install", "download", "wheel", "index", "search"]),
    ("pip3", &["install", "download", "wheel", "index", "search"]),
    (
        "uv",
        &[
            "add", "sync", "lock", "pip", "tool", "run", "python", "init", "venv", "export",
        ],
    ),
    ("poetry", &["install", "add", "update", "lock", "sync"]),
    ("pipenv", &["install", "sync", "update", "lock"]),
    ("conda", &["install", "create", "update", "env"]),
    ("mamba", &["install", "create", "update", "env"]),
    (
        "cargo",
        &[
            "fetch",
            "add",
            "install",
            "update",
            "vendor",
            "generate-lockfile",
            "search",
            "binstall",
        ],
    ),
    ("go", &["get", "install", "download", "tidy", "sync"]),
    ("gem", &["install", "update", "fetch"]),
    ("bundle", &["install", "update", "add"]),
    (
        "composer",
        &["install", "require", "update", "create-project"],
    ),
    ("dotnet", &["add", "restore", "new", "tool"]),
    ("deno", &["install", "add", "cache", "run", "upgrade"]),
    ("brew", &["install", "upgrade", "update", "tap", "fetch"]),
    ("apt", &["install", "update", "upgrade", "download"]),
    (
        "apt-get",
        &["install", "update", "upgrade", "download", "source"],
    ),
    ("choco", &["install", "upgrade"]),
    ("winget", &["install", "upgrade", "download"]),
    ("scoop", &["install", "update", "bucket"]),
];

/// The program a word names, as a shell would find it: without its folder,
/// a Windows extension or letter case (`./node_modules/.bin/NPM.cmd` is npm).
fn program_name(word: &str) -> String {
    let base = word.rsplit(['/', '\\']).next().unwrap_or(word);
    let lower = base.to_ascii_lowercase();
    for extension in [".exe", ".cmd", ".bat", ".ps1"] {
        if let Some(stripped) = lower.strip_suffix(extension) {
            return stripped.to_string();
        }
    }
    lower
}

/// The words of `command` with shell punctuation and quotes as separators:
/// what a package manager and its subcommand look like wherever they sit
/// (after `&&`, inside `bash -c "..."`, behind `sudo` or `env`).
fn loose_words(command: &str) -> Vec<String> {
    command
        .split(|c: char| {
            c.is_whitespace()
                || matches!(
                    c,
                    ';' | '&' | '|' | '(' | ')' | '{' | '}' | '<' | '>' | '"' | '\'' | '`' | '$'
                )
        })
        .filter(|word| !word.is_empty())
        .map(str::to_string)
        .collect()
}

/// Whether `command` may download packages anywhere in it: a program that
/// downloads what it runs ([`DOWNLOAD_RUNNERS`]), a package manager followed
/// later by one of its fetching subcommands ([`FETCHING`]), or `yarn` on its
/// own (which installs). Broad on purpose: Creator mode asks about each.
pub fn mentions_install(command: &str) -> bool {
    let words = loose_words(command);
    for (at, word) in words.iter().enumerate() {
        let program = program_name(word);
        if DOWNLOAD_RUNNERS.contains(&program.as_str()) {
            return true;
        }
        let Some((_, fetching)) = FETCHING.iter().find(|(name, _)| *name == program) else {
            continue;
        };
        let later = words[at + 1..].iter().any(|later| {
            let later = later.to_ascii_lowercase();
            fetching.contains(&later.as_str())
        });
        if later {
            return true;
        }
    }
    bare_yarn(command)
}

/// Whether a part of `command` (between `;`, `&`, `|`, newlines and
/// parentheses) runs `yarn` with nothing but options, which installs.
fn bare_yarn(command: &str) -> bool {
    command
        .split([';', '&', '|', '\n', '\r', '(', ')'])
        .any(|part| {
            let words = loose_words(part);
            words
                .iter()
                .position(|word| program_name(word) == "yarn")
                .is_some_and(|at| words[at + 1..].iter().all(|w| w.starts_with('-')))
        })
}

/// npm install, ci and add: saving and audit options only. `-g`, `--global`,
/// `--prefix`, `--location`, `--registry`, `--userconfig`, `--script-shell`
/// and every other option are not listed, so they make the install ask
/// without the network.
const NPM_INSTALL: Grammar = Grammar {
    flags: &[
        "-D",
        "--save-dev",
        "-S",
        "--save",
        "-E",
        "--save-exact",
        "-O",
        "--save-optional",
        "--no-save",
        "--no-audit",
        "--no-fund",
        "--ignore-scripts",
        "--legacy-peer-deps",
        "--prefer-offline",
        "--production",
    ],
    valued: &["--omit", "--include"],
    positionals: Positionals::Any,
    ..NONE
};

/// pnpm install and add.
const PNPM_INSTALL: Grammar = Grammar {
    flags: &[
        "-D",
        "--save-dev",
        "-E",
        "--save-exact",
        "-O",
        "--save-optional",
        "-P",
        "--prod",
        "--frozen-lockfile",
        "--ignore-scripts",
    ],
    positionals: Positionals::Any,
    ..NONE
};

/// yarn, yarn install and yarn add.
const YARN_INSTALL: Grammar = Grammar {
    flags: &[
        "-D",
        "--dev",
        "-E",
        "--exact",
        "-O",
        "--optional",
        "--frozen-lockfile",
        "--immutable",
        "--ignore-scripts",
    ],
    positionals: Positionals::Any,
    ..NONE
};

/// pip install: a requirements file of the folder or package names. `--user`,
/// `--target`, `--prefix`, `--root`, `--index-url`, `--extra-index-url`,
/// `--editable` and `--src` are not listed.
const PIP_INSTALL: Grammar = Grammar {
    flags: &["-U", "--upgrade", "-q", "--quiet", "--no-cache-dir"],
    valued: &["-r", "--requirement", "--only-binary"],
    positionals: Positionals::Any,
    ..NONE
};

/// cargo fetch: the lock file as it is.
const CARGO_FETCH: Grammar = Grammar {
    flags: &["--locked"],
    ..NONE
};

/// cargo add: crates by name, with their features.
const CARGO_ADD: Grammar = Grammar {
    flags: &["--dev", "--build", "--no-default-features", "--optional"],
    valued: &["-F", "--features"],
    positionals: Positionals::Any,
    ..NONE
};

/// go mod download and go get: modules of the folder's go.mod.
const GO_FETCH: Grammar = Grammar {
    flags: &["-x"],
    positionals: Positionals::Any,
    ..NONE
};

/// Why a package argument is not a plain package name, or `None`. A name
/// may carry a version (`react@18`, `@types/node`), never a URL, a path, an
/// alias to another package (`npm:`), a git source or another registry.
fn package_problem(arg: &str) -> Option<String> {
    if arg.contains(':') {
        return Some(format!(
            "{} names a source (a URL, a path, git or another registry), not a package",
            shown(arg)
        ));
    }
    if arg.starts_with('.') || arg.starts_with('/') || arg.contains('\\') {
        return Some(format!("{} is a path, not a package", shown(arg)));
    }
    if arg.ends_with(".tgz") || arg.ends_with(".tar.gz") || arg.ends_with(".whl") {
        return Some(format!("{} is a package file, not a package", shown(arg)));
    }
    if arg.split('/').count() > 2 && !arg.starts_with('@') {
        return Some(format!("{} is a path, not a package", shown(arg)));
    }
    None
}

/// Whether `command` is one plain package install of a known manager whose
/// options keep it in the folder and whose packages are names: `Ok` with the
/// manager, or `Err` with why not, in plain words. Only such an install gets
/// the network once the user approves it.
pub fn install(command: &str) -> Result<Install, String> {
    if command.chars().count() > MAX_INSTALL_CHARS {
        return Err("it is longer than Shodh reads".to_string());
    }
    if let Some(reason) = special_reason(command) {
        return Err(reason.to_string());
    }
    let words: Vec<&str> = command
        .split([' ', '\t'])
        .filter(|word| !word.is_empty())
        .collect();
    let Some((&program, args)) = words.split_first() else {
        return Err("it is empty".to_string());
    };
    for arg in args {
        if let Some(reason) = outside_project(arg) {
            return Err(reason.to_string());
        }
    }
    let (manager, grammar, rest, packages_are_paths): (&'static str, &Grammar, &[&str], bool) =
        match (program, args) {
            ("npm", [sub, rest @ ..]) if matches!(*sub, "install" | "i" | "ci" | "add") => {
                ("npm", &NPM_INSTALL, rest, false)
            }
            ("pnpm", [sub, rest @ ..]) if matches!(*sub, "install" | "i" | "add") => {
                ("pnpm", &PNPM_INSTALL, rest, false)
            }
            ("yarn", []) => ("yarn", &YARN_INSTALL, &[], false),
            ("yarn", [sub, rest @ ..]) if matches!(*sub, "install" | "add") => {
                ("yarn", &YARN_INSTALL, rest, false)
            }
            ("pip" | "pip3", ["install", rest @ ..]) => ("pip", &PIP_INSTALL, rest, false),
            ("cargo", ["fetch", rest @ ..]) => ("cargo", &CARGO_FETCH, rest, false),
            ("cargo", ["add", rest @ ..]) => ("cargo", &CARGO_ADD, rest, false),
            ("go", ["mod", "download", rest @ ..]) => ("go", &GO_FETCH, rest, false),
            ("go", ["get", rest @ ..]) => ("go", &GO_FETCH, rest, true),
            _ => {
                return Err(format!(
                    "Shodh does not recognise {} as a package install",
                    shown(command)
                ))
            }
        };
    // pip's requirements file is a path of the folder (checked above); every
    // other positional is a package.
    let mut requirement_file = false;
    let mut only_binary = false;
    let mut words = rest.iter();
    while let Some(arg) = words.next() {
        if requirement_file {
            requirement_file = false;
            continue;
        }
        if matches!(*arg, "-r" | "--requirement") {
            requirement_file = true;
            continue;
        }
        // pip's `--only-binary :all:` (wheels only: no build step runs); no
        // other value is read.
        if *arg == "--only-binary" {
            match words.next() {
                Some(&":all:") => {
                    only_binary = true;
                    continue;
                }
                _ => return Err("pip's --only-binary is read only as :all:".to_string()),
            }
        }
        if arg.starts_with('-') {
            continue;
        }
        // Go modules are paths by design (`github.com/x/y`); they still may not
        // name a URL scheme or a local folder.
        let problem = if packages_are_paths {
            (arg.contains(':') || arg.starts_with('.') || arg.contains('\\'))
                .then(|| format!("{} is not a module path", shown(arg)))
        } else {
            package_problem(arg)
        };
        if let Some(problem) = problem {
            return Err(problem);
        }
    }
    accept(manager, rest, grammar)?;
    let scripts = match manager {
        "npm" | "pnpm" | "yarn" => !rest.contains(&"--ignore-scripts"),
        "pip" => !only_binary,
        _ => false,
    };
    Ok(Install { manager, scripts })
}
