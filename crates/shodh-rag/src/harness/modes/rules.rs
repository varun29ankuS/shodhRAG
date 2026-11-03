//! Auto mode's command rules: which commands run without asking.
//!
//! Rules only, no model judges a command. A command runs by itself only when
//! it is one plain command Shodh can read word by word and its program, its
//! subcommand and every option are on the lists below. Everything else asks:
//! a command that is chained, piped, redirected, substituted, quoted,
//! escaped, uses wildcards or `~`, reaches outside the project, runs a
//! program by path or through another program, or is simply not known.
//!
//! How a command is read: any character outside [`PLAIN_CHARS`] makes it ask,
//! before anything else is looked at. That leaves no quoting, escaping,
//! expansion, redirection or command separator, so the words bash runs are
//! exactly the command split at spaces and tabs (bash splits words there and
//! nowhere else). Nothing is unquoted or interpreted: no parser can be fooled
//! by what was never accepted.
//!
//! Options are listed per program and subcommand. An option that is not
//! listed asks, so options that run another program or write outside the
//! project (`cargo --config`, `git -c`, `git diff --output`, `npm
//! --script-shell`, `pytest -p`, `go test -exec`, `rg --pre`, ...) ask without
//! being named here. A positional argument must stay inside the project: no
//! absolute path, drive or `..` part.
//!
//! What a match does not cover is checked by the session: the files the
//! read-only commands open must resolve inside the code folder
//! ([`Rule::reads`]), and git commands run without asking only while the
//! repository's git settings name no program
//! ([`crate::harness::code_mode::git_config_problem`]).

/// Longest command the rules read, in characters.
pub const MAX_RULE_CHARS: usize = 300;

/// The only characters a command the rules accept may have: letters,
/// digits, spaces and tabs, and `- _ . / , : @ ^ +`. None of them is special
/// to bash inside a word.
pub const PLAIN_CHARS: &str = "- _ . / , : @ ^ + (with letters, digits, spaces and tabs)";

fn plain_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(
            c,
            ' ' | '\t' | '-' | '_' | '.' | '/' | ',' | ':' | '@' | '^' | '+'
        )
}

/// A command the rules let run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    /// What kind of command it is, in plain words ("a test command").
    pub what: &'static str,
    /// It runs git: it runs by itself only while the repository's git
    /// settings name no program.
    pub git: bool,
    /// It runs the project's own code (build scripts, package scripts,
    /// conftest.py, test binaries) or tools the project's files choose
    /// (`rust-toolchain.toml`, `.npmrc`, `.yarnrc.yml`). Otherwise it only
    /// inspects: git reads, file reads, a linter that changes nothing.
    pub runs_project_code: bool,
    /// Files and folders the command reads and shows (a read-only command's
    /// arguments, relative to the code folder): each must resolve inside it,
    /// links included.
    pub reads: Vec<String>,
}

/// Whether `command` runs without asking in Auto mode: `Ok` with the rule it
/// matched, or `Err` with why it asks, in plain words ("it pipes output into
/// another program").
pub fn check(command: &str) -> Result<Rule, String> {
    if command.chars().count() > MAX_RULE_CHARS {
        return Err("it is longer than Auto mode reads".to_string());
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
    if program.contains('/') {
        return Err("it runs a program by its path".to_string());
    }
    if let Some(reason) = program_reason(program) {
        return Err(reason.to_string());
    }
    for arg in args {
        // `@file` is a response file to cargo, rustc, pytest (8.2 and later) and many
        // others: arguments read from a file, which no option list could check.
        if arg.starts_with('@') {
            return Err("it reads its arguments from a file (@file)".to_string());
        }
        if let Some(reason) = outside_project(arg) {
            return Err(reason.to_string());
        }
    }
    match program {
        "git" => git(args),
        "cargo" => cargo(args),
        "npm" | "pnpm" | "yarn" => package_script(program, args),
        "pytest" => pytest(args),
        "go" => go(args),
        "ruff" => ruff(args),
        "pwd" | "ls" | "cat" | "head" | "tail" | "wc" | "grep" | "rg" => read_files(program, args),
        _ => Err(format!(
            "Auto mode does not know the program {}",
            shown(program)
        )),
    }
}

/// A word as an explanation shows it: at most 40 characters.
fn shown(word: &str) -> String {
    let kept: String = word.chars().take(40).collect();
    if kept.len() < word.len() {
        format!("{kept}…")
    } else {
        kept
    }
}

/// Why a character of `command` makes it ask, or `None` when every
/// character is plain.
fn special_reason(command: &str) -> Option<&'static str> {
    let odd = command.chars().find(|&c| !plain_char(c))?;
    Some(match odd {
        ';' | '&' => "it runs more than one command, or one in the background",
        '|' => "it pipes output into another program",
        '>' | '<' => "it redirects input or output",
        '$' | '`' => "it uses substitution or variables",
        '(' | ')' | '{' | '}' => "it uses a subshell or grouping",
        '\'' | '"' | '\\' => "it uses quotes or escapes, which Auto mode does not read",
        '*' | '?' | '[' | ']' => "it uses wildcards",
        '~' => "it uses ~ (the home folder)",
        '=' => "it uses =, which Auto mode does not read",
        '\n' | '\r' => "it has more than one line",
        '#' => "it has a comment",
        _ => "it has characters Auto mode does not read",
    })
}

/// Why an argument reaches outside the project: an absolute path, a drive,
/// or a `..` part.
fn outside_project(arg: &str) -> Option<&'static str> {
    let bytes = arg.as_bytes();
    if arg.starts_with('/') {
        return Some("it names a path outside the project");
    }
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return Some("it names a drive");
    }
    if arg.split('/').any(|part| part == "..") {
        return Some("it reaches outside the project with ..");
    }
    None
}

/// Why a program that is never run without asking asks.
fn program_reason(program: &str) -> Option<&'static str> {
    Some(match program {
        "rm" | "rmdir" | "del" | "erase" | "unlink" | "shred" | "rd" => "it deletes files",
        "mv" | "cp" | "ln" | "touch" | "mkdir" | "tee" | "dd" | "truncate" => {
            "it changes files (edits go through Shodh's edit tools)"
        }
        "curl" | "wget" | "ssh" | "scp" | "sftp" | "ftp" | "nc" | "ncat" | "telnet" | "rsync"
        | "http" | "https" | "ping" => "it uses the network",
        "sudo" | "su" | "doas" | "runas" | "gsudo" => "it runs as another user",
        "chmod" | "chown" | "chgrp" | "icacls" | "attrib" | "takeown" => {
            "it changes file permissions"
        }
        "sh" | "bash" | "zsh" | "fish" | "dash" | "ksh" | "pwsh" | "powershell" | "cmd"
        | "python" | "python3" | "py" | "node" | "deno" | "bun" | "ruby" | "perl" | "php"
        | "lua" | "osascript" | "wscript" | "cscript" => "it runs a script or another shell",
        "env" | "exec" | "command" | "builtin" | "time" | "nohup" | "setsid" | "xargs" | "eval"
        | "source" | "." | "nice" | "timeout" | "watch" | "start" | "find" => {
            "it runs another command through a program"
        }
        "npx" | "pnpx" | "bunx" | "pip" | "pip3" | "pipx" | "uv" | "uvx" | "poetry" | "brew"
        | "apt" | "apt-get" | "dnf" | "yum" | "pacman" | "choco" | "winget" | "scoop" | "gem"
        | "conda" | "rustup" => "it installs or downloads packages",
        "kill" | "pkill" | "killall" | "taskkill" | "shutdown" | "reboot" | "systemctl"
        | "launchctl" | "schtasks" | "crontab" | "sc" | "reg" => {
            "it changes the system or other programs"
        }
        _ => return None,
    })
}

/// Which positional arguments a subcommand takes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Positionals {
    None,
    Any,
}

/// What a program or subcommand accepts after its name.
struct Grammar {
    /// Options taking no value.
    flags: &'static [&'static str],
    /// Options taking the next word as their value.
    valued: &'static [&'static str],
    /// Letters that may be combined into one short option (`-la`).
    combined: &'static str,
    /// `-5` (a count) is accepted.
    numeric: bool,
    positionals: Positionals,
    /// What may follow `--`; `None`: `--` is not accepted.
    after_dashes: Option<&'static Grammar>,
}

const NONE: Grammar = Grammar {
    flags: &[],
    valued: &[],
    combined: "",
    numeric: false,
    positionals: Positionals::None,
    after_dashes: None,
};

/// After `--`: only paths.
const PATHS: Grammar = Grammar {
    positionals: Positionals::Any,
    ..NONE
};

/// A value: not an option.
fn plain_value(word: &str) -> bool {
    !word.is_empty() && !word.starts_with('-')
}

/// Checks `args` against `grammar`; the positional arguments are returned.
fn accept<'a>(program: &str, args: &[&'a str], grammar: &Grammar) -> Result<Vec<&'a str>, String> {
    let mut positionals = Vec::new();
    let mut at = 0;
    while at < args.len() {
        let word = args[at];
        at += 1;
        if word == "--" {
            let Some(rest) = grammar.after_dashes else {
                return Err(format!("Auto mode does not read -- after {program}"));
            };
            positionals.extend(accept(program, &args[at..], rest)?);
            return Ok(positionals);
        }
        if !word.starts_with('-') || word == "-" {
            if grammar.positionals == Positionals::None || word == "-" {
                return Err(format!(
                    "Auto mode does not run {program} with {}",
                    shown(word)
                ));
            }
            positionals.push(word);
            continue;
        }
        if grammar.flags.contains(&word) {
            continue;
        }
        if grammar.valued.contains(&word) {
            match args.get(at) {
                Some(value) if plain_value(value) => {
                    at += 1;
                    continue;
                }
                _ => {
                    return Err(format!(
                        "the option {} of {program} needs a plain value",
                        shown(word)
                    ))
                }
            }
        }
        let short = word.strip_prefix('-').filter(|rest| !rest.starts_with('-'));
        if let Some(letters) = short {
            if grammar.numeric && letters.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            if !grammar.combined.is_empty() && letters.chars().all(|c| grammar.combined.contains(c))
            {
                continue;
            }
        }
        return Err(format!(
            "Auto mode does not know the option {} of {program}",
            shown(word)
        ));
    }
    Ok(positionals)
}

/// The rule for a subcommand from `table`, by the first argument.
fn subcommand<'a, 'b>(
    program: &str,
    args: &'a [&'b str],
    table: &[(&str, &'static str, &'static Grammar)],
) -> Result<(&'static str, &'a [&'b str], &'static Grammar), String> {
    let Some((&first, rest)) = args.split_first() else {
        return Err(format!("Auto mode does not run {program} on its own"));
    };
    match table.iter().find(|(name, _, _)| *name == first) {
        Some((_, what, grammar)) => Ok((*what, rest, *grammar)),
        None => Err(match program_subcommand_reason(program, first) {
            Some(reason) => reason.to_string(),
            None => format!("Auto mode does not know {program} {}", shown(first)),
        }),
    }
}

/// Why a known risky subcommand asks.
fn program_subcommand_reason(program: &str, sub: &str) -> Option<&'static str> {
    Some(match (program, sub) {
        ("git", "push" | "pull" | "fetch" | "clone" | "ls-remote" | "submodule") => {
            "it uses the network"
        }
        (
            "git",
            "reset" | "clean" | "checkout" | "switch" | "restore" | "rebase" | "merge" | "commit"
            | "stash" | "rm" | "mv" | "add" | "cherry-pick" | "revert" | "tag" | "am" | "apply"
            | "gc" | "prune" | "worktree" | "update-ref" | "filter-branch" | "replace",
        ) => "it changes the repository",
        ("git", "config") => "it changes git's settings",
        (
            _,
            "install" | "i" | "add" | "ci" | "update" | "upgrade" | "uninstall" | "remove" | "get"
            | "fetch" | "dlx" | "exec" | "x",
        ) => "it installs or downloads packages",
        (_, "publish" | "login" | "logout" | "owner" | "yank" | "deploy") => {
            "it reaches an account or registry"
        }
        (_, "run") if program == "cargo" || program == "go" => "it runs the program it builds",
        (_, "clean") => "it deletes build output",
        _ => return None,
    })
}

// ── git ────────────────────────────────────────────────────────────────────

const GIT_STATUS: Grammar = Grammar {
    flags: &[
        "-s",
        "-b",
        "-sb",
        "--short",
        "--branch",
        "--porcelain",
        "--long",
        "-u",
        "-uno",
        "-unormal",
        "-uall",
        "-v",
        "--ignored",
        "--no-renames",
    ],
    positionals: Positionals::Any,
    after_dashes: Some(&PATHS),
    ..NONE
};

const GIT_DIFF: Grammar = Grammar {
    flags: &[
        "--stat",
        "--shortstat",
        "--numstat",
        "--name-only",
        "--name-status",
        "--cached",
        "--staged",
        "--check",
        "--no-color",
        "--color",
        "--word-diff",
        "-w",
        "-b",
        "--ignore-all-space",
        "--ignore-space-change",
        "--patch",
        "-p",
        "--raw",
        "--summary",
        "--minimal",
        "--histogram",
        "--patience",
        "-R",
        "--no-renames",
        "-M",
    ],
    positionals: Positionals::Any,
    after_dashes: Some(&PATHS),
    ..NONE
};

const GIT_LOG: Grammar = Grammar {
    flags: &[
        "--oneline",
        "--stat",
        "--shortstat",
        "--name-only",
        "--name-status",
        "-p",
        "--patch",
        "--graph",
        "--decorate",
        "--all",
        "--no-merges",
        "--merges",
        "--reverse",
        "--first-parent",
        "--abbrev-commit",
        "--no-color",
        "--date-order",
        "--topo-order",
    ],
    valued: &[
        "-n",
        "--max-count",
        "--author",
        "--since",
        "--until",
        "--grep",
        "--skip",
    ],
    numeric: true,
    positionals: Positionals::Any,
    after_dashes: Some(&PATHS),
    ..NONE
};

const GIT_SHOW: Grammar = Grammar {
    flags: &[
        "--stat",
        "--shortstat",
        "--name-only",
        "--name-status",
        "--oneline",
        "-s",
        "--no-patch",
        "--patch",
        "-p",
        "--no-color",
        "--abbrev-commit",
    ],
    positionals: Positionals::Any,
    after_dashes: Some(&PATHS),
    ..NONE
};

/// Lists branches only: a name would create one.
const GIT_BRANCH: Grammar = Grammar {
    flags: &[
        "-a",
        "--all",
        "-r",
        "--remotes",
        "-v",
        "-vv",
        "--verbose",
        "--list",
        "--show-current",
        "--no-color",
    ],
    ..NONE
};

const GIT_REV_PARSE: Grammar = Grammar {
    flags: &[
        "--abbrev-ref",
        "--short",
        "--show-toplevel",
        "--show-prefix",
        "--is-inside-work-tree",
        "--verify",
        "--symbolic-full-name",
        "-q",
        "--quiet",
    ],
    positionals: Positionals::Any,
    ..NONE
};

const GIT_LS_FILES: Grammar = Grammar {
    flags: &[
        "-c",
        "--cached",
        "-m",
        "--modified",
        "-o",
        "--others",
        "-d",
        "--deleted",
        "--exclude-standard",
        "-s",
        "--stage",
    ],
    positionals: Positionals::Any,
    after_dashes: Some(&PATHS),
    ..NONE
};

const GIT_BLAME: Grammar = Grammar {
    flags: &["-w", "-e", "-s", "-l", "-M"],
    valued: &["-L"],
    positionals: Positionals::Any,
    after_dashes: Some(&PATHS),
    ..NONE
};

/// Lists remotes only.
const GIT_REMOTE: Grammar = Grammar {
    flags: &["-v", "--verbose"],
    ..NONE
};

const GIT_READS: &str = "a git command that only reads";

/// git: only subcommands that read, each with its own options. The first
/// argument must be the subcommand: global options (`-c`, `-C`,
/// `--git-dir`, `--exec-path`) ask.
fn git(args: &[&str]) -> Result<Rule, String> {
    let table: [(&str, &'static str, &'static Grammar); 9] = [
        ("status", GIT_READS, &GIT_STATUS),
        ("diff", GIT_READS, &GIT_DIFF),
        ("log", GIT_READS, &GIT_LOG),
        ("show", GIT_READS, &GIT_SHOW),
        ("branch", GIT_READS, &GIT_BRANCH),
        ("rev-parse", GIT_READS, &GIT_REV_PARSE),
        ("ls-files", GIT_READS, &GIT_LS_FILES),
        ("blame", GIT_READS, &GIT_BLAME),
        ("remote", GIT_READS, &GIT_REMOTE),
    ];
    if args.first().is_some_and(|first| first.starts_with('-')) {
        return Err("it gives git options before the subcommand".to_string());
    }
    let (what, rest, grammar) = subcommand("git", args, &table)?;
    accept("git", rest, grammar)?;
    Ok(Rule {
        what,
        git: true,
        runs_project_code: false,
        reads: Vec::new(),
    })
}

// ── cargo ──────────────────────────────────────────────────────────────────

const CARGO_FLAGS: &[&str] = &[
    "--release",
    "--workspace",
    "--all",
    "--all-targets",
    "--all-features",
    "--no-default-features",
    "--lib",
    "--bins",
    "--tests",
    "--examples",
    "--benches",
    "--locked",
    "--offline",
    "--frozen",
    "-q",
    "--quiet",
    "-v",
    "-vv",
    "--verbose",
    "--keep-going",
];

const CARGO_VALUED: &[&str] = &[
    "-p",
    "--package",
    "--test",
    "--bin",
    "--example",
    "--bench",
    "--features",
    "-F",
    "-j",
    "--jobs",
    "--exclude",
];

/// The test binary's own options (libtest), after `--`.
const LIBTEST: Grammar = Grammar {
    flags: &[
        "--nocapture",
        "--no-capture",
        "--ignored",
        "--include-ignored",
        "--exact",
        "--quiet",
        "-q",
        "--show-output",
        "--list",
    ],
    valued: &["--test-threads", "--skip"],
    positionals: Positionals::Any,
    ..NONE
};

const CARGO_TEST: Grammar = Grammar {
    flags: &[
        "--release",
        "--workspace",
        "--all",
        "--all-targets",
        "--all-features",
        "--no-default-features",
        "--lib",
        "--bins",
        "--tests",
        "--examples",
        "--benches",
        "--locked",
        "--offline",
        "--frozen",
        "-q",
        "--quiet",
        "-v",
        "-vv",
        "--verbose",
        "--no-fail-fast",
        "--no-run",
        "--doc",
    ],
    valued: CARGO_VALUED,
    positionals: Positionals::Any,
    after_dashes: Some(&LIBTEST),
    ..NONE
};

const CARGO_BUILD: Grammar = Grammar {
    flags: CARGO_FLAGS,
    valued: CARGO_VALUED,
    ..NONE
};

/// Lint levels for clippy, after `--`.
const CLIPPY_LINTS: Grammar = Grammar {
    valued: &["-D", "-W", "-A", "--deny", "--warn", "--allow"],
    ..NONE
};

const CARGO_CLIPPY: Grammar = Grammar {
    flags: CARGO_FLAGS,
    valued: CARGO_VALUED,
    after_dashes: Some(&CLIPPY_LINTS),
    ..NONE
};

/// rustfmt's options, after `--`.
const RUSTFMT: Grammar = Grammar {
    flags: &["--check", "-v", "--verbose", "-q", "--quiet"],
    ..NONE
};

const CARGO_FMT: Grammar = Grammar {
    flags: &["--check", "--all", "-q", "--quiet", "-v", "--verbose"],
    valued: &["-p", "--package"],
    after_dashes: Some(&RUSTFMT),
    ..NONE
};

/// cargo: tests, builds, checks and linters of the project. Options that
/// change where cargo reads its settings or writes (`--config`,
/// `--manifest-path`, `--target-dir`, `-Z`, `-C`) are not listed, so they ask.
fn cargo(args: &[&str]) -> Result<Rule, String> {
    let table: [(&str, &'static str, &'static Grammar); 5] = [
        ("test", "a test command", &CARGO_TEST),
        ("build", "a build command", &CARGO_BUILD),
        ("check", "a build command", &CARGO_BUILD),
        ("clippy", "a linter", &CARGO_CLIPPY),
        ("fmt", "a formatter", &CARGO_FMT),
    ];
    if args.first().is_some_and(|first| first.starts_with('+')) {
        return Err("it picks another toolchain".to_string());
    }
    let (what, rest, grammar) = subcommand("cargo", args, &table)?;
    accept("cargo", rest, grammar)?;
    Ok(Rule {
        what,
        git: false,
        runs_project_code: true,
        reads: Vec::new(),
    })
}

// ── package scripts ────────────────────────────────────────────────────────

/// Scripts of `package.json` that run by themselves: these names, or one of
/// them followed by `:` and more (`test:unit`). A script that deploys or
/// publishes has another name, so it asks.
const SCRIPT_NAMES: [&str; 6] = ["test", "build", "lint", "check", "typecheck", "type-check"];

fn known_script(name: &str) -> bool {
    SCRIPT_NAMES.iter().any(|base| {
        name == *base
            || name
                .strip_prefix(base)
                .and_then(|rest| rest.strip_prefix(':'))
                .is_some_and(|rest| !rest.is_empty() && plain_value(rest))
    })
}

/// npm, pnpm and yarn: `<program> test` or `<program> run <script>` with a
/// known script name ([`SCRIPT_NAMES`]) and nothing else: an option such as
/// `--script-shell` or `--prefix` asks.
fn package_script(program: &str, args: &[&str]) -> Result<Rule, String> {
    let script = match args {
        ["test"] => "test",
        ["run", name] => *name,
        [first, ..] if first.starts_with('-') => {
            return Err(format!(
                "Auto mode does not know the option {} of {program}",
                shown(first)
            ))
        }
        [first, ..] if *first != "test" && *first != "run" => {
            return Err(match program_subcommand_reason(program, first) {
                Some(reason) => reason.to_string(),
                None => format!("Auto mode does not know {program} {}", shown(first)),
            })
        }
        _ => {
            return Err(format!(
                "Auto mode runs only {program} test or {program} run <script>, with nothing else"
            ))
        }
    };
    if !known_script(script) {
        return Err(format!(
            "Auto mode runs only the scripts test, build, lint, check and typecheck, not {}",
            shown(script)
        ));
    }
    Ok(Rule {
        what: "a project script (test, build, lint or check)",
        git: false,
        runs_project_code: true,
        reads: Vec::new(),
    })
}

// ── pytest, go, ruff ───────────────────────────────────────────────────────

/// pytest: plugins (`-p`), another config file (`-c`), ini overrides (`-o`)
/// and other folders (`--rootdir`, `--basetemp`) are not listed, so they ask.
const PYTEST: Grammar = Grammar {
    flags: &[
        "-q",
        "-v",
        "-vv",
        "-x",
        "-s",
        "--lf",
        "--ff",
        "--last-failed",
        "--failed-first",
        "--co",
        "--collect-only",
        "-ra",
        "-rA",
        "--no-header",
        "--disable-warnings",
        "--sw",
        "--stepwise",
        "--exitfirst",
    ],
    valued: &["-k", "-m", "--maxfail", "--tb", "-n", "--durations"],
    positionals: Positionals::Any,
    ..NONE
};

fn pytest(args: &[&str]) -> Result<Rule, String> {
    accept("pytest", args, &PYTEST)?;
    Ok(Rule {
        what: "a test command",
        git: false,
        runs_project_code: true,
        reads: Vec::new(),
    })
}

/// go: `-exec`, `-toolexec`, `-o`, `-C`, `-modfile` and `-overlay` are not
/// listed, so they ask.
const GO: Grammar = Grammar {
    flags: &["-v", "-race", "-short", "-cover", "-failfast", "-json"],
    valued: &["-run", "-count", "-timeout", "-bench", "-p", "-tags"],
    positionals: Positionals::Any,
    ..NONE
};

fn go(args: &[&str]) -> Result<Rule, String> {
    let table: [(&str, &'static str, &'static Grammar); 3] = [
        ("test", "a test command", &GO),
        ("build", "a build command", &GO),
        ("vet", "a linter", &GO),
    ];
    let (what, rest, grammar) = subcommand("go", args, &table)?;
    accept("go", rest, grammar)?;
    Ok(Rule {
        what,
        git: false,
        runs_project_code: true,
        reads: Vec::new(),
    })
}

/// ruff: `--config` is not listed, so it asks.
const RUFF: Grammar = Grammar {
    flags: &[
        "--fix",
        "--diff",
        "--check",
        "-q",
        "--quiet",
        "--statistics",
        "--show-fixes",
        "--no-cache",
    ],
    positionals: Positionals::Any,
    ..NONE
};

fn ruff(args: &[&str]) -> Result<Rule, String> {
    let table: [(&str, &'static str, &'static Grammar); 2] = [
        ("check", "a linter", &RUFF),
        ("format", "a formatter", &RUFF),
    ];
    let (what, rest, grammar) = subcommand("ruff", args, &table)?;
    accept("ruff", rest, grammar)?;
    // Checking without fixing, or formatting with --check or --diff, changes nothing.
    let reads_only = match args.first().copied() {
        Some("check") => !rest.contains(&"--fix"),
        Some("format") => rest.contains(&"--check") || rest.contains(&"--diff"),
        _ => false,
    };
    Ok(Rule {
        what,
        git: false,
        runs_project_code: !reads_only,
        reads: Vec::new(),
    })
}

// ── commands that only read files ──────────────────────────────────────────

const LS: Grammar = Grammar {
    flags: &[
        "--all",
        "--almost-all",
        "--human-readable",
        "--reverse",
        "--recursive",
    ],
    combined: "laAhrtSR1dFG",
    positionals: Positionals::Any,
    ..NONE
};

const CAT: Grammar = Grammar {
    combined: "nAbsvET",
    positionals: Positionals::Any,
    ..NONE
};

/// head and tail; `tail -f` (which never ends) is not listed.
const HEAD_TAIL: Grammar = Grammar {
    valued: &["-n", "-c"],
    numeric: true,
    positionals: Positionals::Any,
    ..NONE
};

const WC: Grammar = Grammar {
    combined: "lwcm",
    positionals: Positionals::Any,
    ..NONE
};

/// grep; `-R` (which follows links), `-f` and `--include` are not listed.
const GREP: Grammar = Grammar {
    combined: "rnilvwcEFHhoqsxL",
    valued: &["-e", "-m", "-A", "-B", "-C"],
    positionals: Positionals::Any,
    ..NONE
};

/// ripgrep; `--pre` (which runs a program on each file), `-z` (which runs
/// decompressors) and `-L` (which follows links) are not listed.
const RG: Grammar = Grammar {
    flags: &[
        "--hidden",
        "--files",
        "--no-ignore",
        "--count",
        "--fixed-strings",
        "--ignore-case",
        "--line-number",
        "--files-with-matches",
        "--word-regexp",
        "--smart-case",
    ],
    valued: &["-g", "--glob", "-t", "--type", "-e", "-m", "-A", "-B", "-C"],
    combined: "inlwcFSuvo",
    positionals: Positionals::Any,
    ..NONE
};

/// Commands that only read files and folders of the project and show them.
/// What they read must resolve inside the code folder ([`Rule::reads`]):
/// every positional argument (grep and rg: every one after the pattern; the
/// pattern given with `-e` leaves them all as paths).
fn read_files(program: &str, args: &[&str]) -> Result<Rule, String> {
    let grammar = match program {
        "pwd" => &NONE,
        "ls" => &LS,
        "cat" => &CAT,
        "head" | "tail" => &HEAD_TAIL,
        "wc" => &WC,
        "grep" => &GREP,
        _ => &RG,
    };
    let positionals = accept(program, args, grammar)?;
    // `rg --files` takes no pattern: every positional is a path.
    let pattern_given = args.contains(&"-e") || (program == "rg" && args.contains(&"--files"));
    let paths: Vec<String> = match program {
        "grep" | "rg" if !pattern_given => {
            positionals.iter().skip(1).map(|p| p.to_string()).collect()
        }
        _ => positionals.iter().map(|p| p.to_string()).collect(),
    };
    Ok(Rule {
        what: "a command that only reads files",
        git: false,
        runs_project_code: false,
        reads: paths,
    })
}
