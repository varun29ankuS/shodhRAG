use std::sync::atomic::AtomicUsize;

use super::*;

#[test]
fn arguments_are_exactly_a_command_and_an_optional_time_limit() {
    let parsed = parse_args(&json!({"command": "cargo test", "i": "run the tests"})).unwrap();
    assert_eq!(
        parsed,
        CommandArgs {
            command: "cargo test".into(),
            timeout_secs: None
        }
    );
    assert_eq!(parsed.input(), json!({"command": "cargo test"}));
    let timed = parse_args(&json!({"command": "pytest", "timeout_secs": 90})).unwrap();
    assert_eq!(timed.timeout_secs, Some(90));
    assert_eq!(
        timed.input(),
        json!({"command": "pytest", "timeout_secs": 90})
    );
    assert_eq!(asked_timeout(&timed.input()), Some(90));
    // Some models write whole numbers as decimals, or send unused optionals as null.
    let decimal = parse_args(&json!({"command": "pytest", "timeout_secs": 60.0})).unwrap();
    assert_eq!(decimal.timeout_secs, Some(60));
    let null = parse_args(&json!({"command": "pytest", "timeout_secs": null})).unwrap();
    assert_eq!(null.timeout_secs, None);

    for (args, why) in [
        (json!("ls"), "an object"),
        (json!({}), "needs a command"),
        (json!({"command": "   "}), "needs a command"),
        (json!({"command": 3}), "must be text"),
        (json!({"command": "ls\u{0}rm"}), "U+0000"),
        (json!({"command": "ls", "cwd": ".."}), "not cwd"),
        (json!({"command": "ls", "async": true}), "not async"),
        (json!({"command": "ls", "timeout_secs": 0}), "whole number"),
        (json!({"command": "ls", "timeout_secs": -5}), "whole number"),
        (
            json!({"command": "ls", "timeout_secs": 1.5}),
            "whole number",
        ),
        (
            json!({"command": "ls", "timeout_secs": "60"}),
            "whole number",
        ),
        (
            json!({"command": "ls", "timeout_secs": MAX_ASKED_SECS + 1}),
            "whole number",
        ),
    ] {
        let error = parse_args(&args).unwrap_err();
        assert!(error.contains(why), "{args}: {error}");
    }
    let long = "x".repeat(MAX_COMMAND_CHARS + 1);
    assert!(parse_args(&json!({ "command": long }))
        .unwrap_err()
        .contains("script file"));
    let longest = "é".repeat(MAX_COMMAND_CHARS);
    assert!(parse_args(&json!({ "command": longest })).is_ok());
}

#[test]
fn a_command_is_never_longer_than_its_approval_prompt_shows() {
    // The prompt shows MAX_PREVIEW_CHARS of a command in full; a command
    // longer than that is refused before anything is asked.
    assert_eq!(
        MAX_COMMAND_CHARS,
        super::super::code_mode::MAX_PREVIEW_CHARS
    );
    let padded = format!("echo ok{}rm -rf ~", " x".repeat(1_000));
    assert!(padded.chars().count() > MAX_COMMAND_CHARS);
    assert!(parse_args(&json!({ "command": padded })).is_err());
    let longest = format!("echo {}", "y".repeat(MAX_COMMAND_CHARS - 5));
    let parsed = parse_args(&json!({ "command": longest.clone() })).unwrap();
    let request = super::super::code_mode::GuardRequest {
        ui_id: None,
        tool_call_id: "call".into(),
        tool: RUN_COMMAND.into(),
        input: parsed.input(),
        target: None,
    };
    let approval = super::super::code_mode::approval_for(
        &request,
        None,
        super::super::code_mode::ChangeTarget::Unknown,
    );
    assert_eq!(approval.preview["command"], longest, "shown in full");
}

#[test]
fn commands_cannot_hide_text_from_their_approval_prompt() {
    for hidden in [
        "\u{202E}",
        "\u{2066}",
        "\u{200B}",
        "\u{200D}",
        "\u{2060}",
        "\u{FEFF}",
        "\u{00AD}",
        "\u{1B}[8m",
        "\r",
        "\u{7}",
        "\u{E0041}",
    ] {
        let command = format!("echo safe{hidden}; rm -rf ~");
        let error = parse_args(&json!({ "command": command })).unwrap_err();
        assert!(error.contains("U+"), "{hidden:?}: {error}");
    }
    // Long runs of blanks would push the rest of the command out of sight.
    let spaced = format!("echo ok{}rm -rf ~", " ".repeat(MAX_BLANK_RUN + 1));
    assert!(parse_args(&json!({ "command": spaced }))
        .unwrap_err()
        .contains("blank characters"));
    let lines = format!("echo ok{}rm -rf ~", "\n".repeat(MAX_BLANK_RUN + 1));
    assert!(parse_args(&json!({ "command": lines })).is_err());
    // Ordinary text, tabs, line breaks and non-Latin letters are fine.
    let blanks = format!("echo a{}b", " ".repeat(MAX_BLANK_RUN));
    for fine in [
        "printf 'a\tb\n' && echo done",
        "cat <<'EOF'\nline one\n\nline two\nEOF",
        "echo 'é✓ 日本語 مرحبا'",
        blanks.as_str(),
    ] {
        assert!(parse_args(&json!({ "command": fine })).is_ok(), "{fine:?}");
    }
}

#[test]
fn the_tool_is_described_with_its_schema() {
    let definition = definition();
    assert_eq!(definition.name, RUN_COMMAND);
    assert_eq!(definition.load_mode, ToolLoadMode::Essential);
    for words in [
        "bash",
        "code folder",
        "background",
        "time limit",
        "API keys and tokens are not passed",
        "scheduled tasks",
    ] {
        assert!(definition.description.contains(words), "{words}");
    }
    let schema = schema();
    assert_eq!(schema["required"], json!(["command"]));
    assert_eq!(schema["additionalProperties"], false);
    // The runtime's copy lets the optional limit be null.
    assert_eq!(
        definition.parameters["properties"]["command"]["type"],
        "string"
    );
}

fn settings(timeout: u64, memory_mb: u64, cpu: u32) -> CodeSettings {
    CodeSettings {
        command_timeout_secs: timeout,
        memory_limit_mb: memory_mb,
        cpu_percent: cpu,
        ..CodeSettings::default()
    }
}

#[test]
fn a_call_may_ask_for_less_time_never_more() {
    let workspace = settings(600, 4096, 80);
    let own = CommandLimits::for_call(&workspace, None);
    assert_eq!(own.time, Duration::from_secs(600));
    assert!(!own.time_asked);
    assert_eq!(own.memory_bytes, 4096 * 1024 * 1024);
    assert_eq!(own.cpu_percent, 80);
    assert_eq!(own.max_processes, MAX_PROCESSES);
    let shorter = CommandLimits::for_call(&workspace, Some(90));
    assert_eq!(shorter.time, Duration::from_secs(90));
    assert!(shorter.time_asked);
    let longer = CommandLimits::for_call(&workspace, Some(7_200));
    assert_eq!(longer.time, Duration::from_secs(600));
    assert!(!longer.time_asked);

    let rows = own.prompt_rows();
    assert_eq!(
        rows[0],
        (
            "time_limit",
            "10 minutes (the workspace's limit)".to_string()
        )
    );
    assert_eq!(rows[1].0, "limits");
    // Only what this system enforces is promised.
    let limits = &rows[1].1;
    assert!(limits.ends_with(STOPPED_AT_END), "{limits}");
    assert!(STOPPED_AT_END.contains("except programs it has the system start"));
    #[cfg(windows)]
    assert_eq!(
        limits.as_str(),
        format!(
            "Memory 4 GB for all its processes (an allocation beyond it fails), CPU 80%, at \
             most 64 processes at once. {STOPPED_AT_END}"
        )
    );
    #[cfg(target_os = "macos")]
    {
        assert_eq!(
            limits.as_str(),
            format!(
                "Memory 4 GB for all its processes and 64 processes at once (the command is \
                 stopped beyond either), lower CPU priority (macOS has no CPU cap). \
                 {STOPPED_AT_END}"
            )
        );
        assert!(!limits.contains("80%"), "macOS enforces no CPU cap");
    }
    assert_eq!(
        shorter.prompt_rows()[0].1,
        "90 seconds (asked by the assistant)"
    );
    let odd = CommandLimits::for_call(&settings(61, 1500, 50), None);
    assert_eq!(odd.time_text(), "61 seconds");
    assert!(odd.prompt_rows()[1].1.starts_with("Memory 1500 MB"));
    assert_eq!(
        CommandLimits::for_call(&settings(60, 1024, 10), None).time_text(),
        "1 minute"
    );
}

#[test]
fn output_keeps_its_start_and_end_within_the_cap() {
    let mut output = CappedOutput::new(10);
    output.push(b"abc");
    assert_eq!(output.text(), "abc");
    assert_eq!(output.dropped(), 0);
    output.push(b"defghij");
    output.push(b"klmnop");
    // Five bytes of head, five of tail, the middle counted.
    assert_eq!(output.dropped(), 6);
    assert_eq!(output.text(), "abcde\n[… 6 bytes cut …]\nlmnop");
    // One big push keeps its own end.
    let mut big = CappedOutput::new(8);
    big.push(&[b'x'; 100]);
    assert_eq!(big.dropped(), 92);
    assert!(big.text().ends_with("xxxx"));
    // Bytes that are not UTF-8 are shown, not dropped.
    let mut bytes = CappedOutput::new(100);
    bytes.push(&[0xff, b'a']);
    assert_eq!(bytes.text(), "\u{fffd}a");

    assert_eq!(head_and_tail("short", 3, 3), "short");
    assert_eq!(
        head_and_tail("abcdefghij", 2, 3),
        "ab\n[… 5 characters cut …]\nhij"
    );
}

#[test]
fn live_output_is_sent_whole_characters_at_a_time_and_bounded() {
    let mut live = LiveOutput::default();
    assert_eq!(live.take(false), None);
    let check = "✓".as_bytes();
    live.push(b"ok ");
    live.push(&check[..2]);
    // The incomplete character waits for the rest.
    assert_eq!(live.take(false).as_deref(), Some("ok "));
    assert_eq!(live.take(false), None);
    live.push(&check[2..]);
    assert_eq!(live.take(false).as_deref(), Some("✓"));
    // Beyond the chunk only the newest is kept, and the gap is said.
    live.push(&vec![b'a'; LIVE_CHUNK_BYTES + 10]);
    let text = live.take(false).unwrap();
    assert!(text.starts_with("[… 10 bytes not shown …]\n"), "{text}");
    assert_eq!(
        text.len() - "[… 10 bytes not shown …]\n".len(),
        LIVE_CHUNK_BYTES
    );
    // The last flush sends what is left as it is.
    live.push(&check[..1]);
    assert_eq!(live.take(true).as_deref(), Some("\u{fffd}"));
}

#[test]
fn terminal_control_is_removed_from_output() {
    let mut filter = OutputFilter::default();
    let mut clean = |bytes: &[u8]| String::from_utf8(filter.clean(bytes)).unwrap();
    assert_eq!(clean(b"\x1b[1;32mok\x1b[0m done\n"), "ok done\n");
    // A sequence split across reads.
    assert_eq!(clean(b"a\x1b[3"), "a");
    assert_eq!(clean(b"1mred\x1b[0m\n"), "red\n");
    // A window title (OSC), ended by BEL or by ESC \.
    assert_eq!(clean(b"\x1b]0;title\x07x\x1b]2;t\x1b\\y\n"), "xy\n");
    // Progress redrawn with a carriage return becomes lines; CRLF is one break.
    assert_eq!(clean(b"10%\r50%\r100%\r\n"), "10%\n50%\n100%\n");
    assert_eq!(clean(b"line\r"), "line");
    assert_eq!(filter.finish(), b"\n");
    assert_eq!(filter.finish(), b"");

    // A character-set choice (`ESC ( B`) leaves no stray character.
    let mut filter = OutputFilter::default();
    assert_eq!(
        String::from_utf8(filter.clean(b"a\x1b(Bb\x1b)0c\n")).unwrap(),
        "abc\n"
    );
    // An operating-system command that never ends cannot swallow the rest of
    // the output: the line ends it...
    let mut filter = OutputFilter::default();
    assert_eq!(
        String::from_utf8(filter.clean(b"\x1b]0;never ended\nstill here\n")).unwrap(),
        "\nstill here\n"
    );
    // ...and so does its length.
    let mut filter = OutputFilter::default();
    let mut long = b"\x1b]".to_vec();
    long.extend(std::iter::repeat_n(b'x', MAX_OSC_BYTES + 10));
    let kept = filter.clean(&long);
    assert!(!kept.is_empty() && kept.len() <= 10, "{}", kept.len());
    assert_eq!(filter.clean(b"after"), b"after");
    // A control sequence cut by text is not one: the text stays.
    let mut filter = OutputFilter::default();
    assert_eq!(
        String::from_utf8(filter.clean(b"\x1b[12\nnext")).unwrap(),
        "\nnext"
    );
}

#[test]
fn outcomes_tell_the_model_and_the_user_how_the_command_ended() {
    let limits = CommandLimits::for_call(&settings(600, 4096, 80), Some(30));
    let done = CommandOutcome {
        state: CommandState::Done,
        exit_code: Some(1),
        note: Some(
            "2 processes it started were still running when it ended and were stopped.".into(),
        ),
        output: "test a ... FAILED\n".into(),
        duration: Duration::from_millis(1_500),
        escaped: false,
    };
    assert!(!done.succeeded());
    assert_eq!(done.summary(&limits), "Exit 1 · 1 line of output");
    let text = done.text_for_model(&limits);
    assert!(text.starts_with("Exit code 1 after 1.5 s."), "{text}");
    assert!(text.contains("were stopped"), "{text}");
    assert!(
        text.ends_with("Output (stdout and stderr):\ntest a ... FAILED\n"),
        "{text}"
    );

    let stopped = CommandOutcome {
        state: CommandState::Stopped,
        exit_code: None,
        note: None,
        output: String::new(),
        duration: Duration::from_secs(4),
        escaped: false,
    };
    assert_eq!(stopped.summary(&limits), "Stopped by you");
    let text = stopped.text_for_model(&limits);
    assert!(text.contains("The user stopped this command"), "{text}");
    assert!(text.contains("Do not run it again"), "{text}");
    assert!(text.ends_with("No output."), "{text}");

    let timed_out = CommandOutcome {
        state: CommandState::TimedOut,
        ..stopped.clone()
    };
    assert_eq!(timed_out.summary(&limits), "Timed out after 30 seconds");
    assert!(timed_out
        .text_for_model(&limits)
        .contains("reached its time limit (30 seconds)"));

    let killed = CommandOutcome {
        state: CommandState::Killed,
        note: Some("the memory limit (4096 MB for all its processes)".into()),
        ..stopped.clone()
    };
    assert_eq!(
        killed.summary(&limits),
        "Killed: the memory limit (4096 MB for all its processes)"
    );
    let refused = CommandOutcome::refused(NO_GIT_BASH);
    assert_eq!(refused.summary(&limits), format!("Refused: {NO_GIT_BASH}"));
    let text = refused.text_for_model(&limits);
    assert_eq!(text, format!("The command was not run: {NO_GIT_BASH}"));

    let long = CommandOutcome {
        output: "y".repeat(MODEL_HEAD_CHARS + MODEL_TAIL_CHARS + 100),
        ..done
    };
    let text = long.text_for_model(&limits);
    assert!(text.contains("[… 100 characters cut …]"), "{text}");
}

#[test]
fn commands_are_quoted_for_git_bash_as_one_argument() {
    assert_eq!(msys_quoted("ls"), "\"ls\"");
    assert_eq!(
        msys_quoted(r#"echo "a b" 'c' \d *"#),
        r#""echo \"a b\" 'c' \\d *""#
    );
    // Text starting like a drive is kept away from MSYS2's path conversion.
    assert_eq!(msys_quoted(r"C:\tools\x.exe"), r#"" C:\\tools\\x.exe""#);
    assert_eq!(msys_quoted("c:"), "\" c:\"");
}

fn touch(path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, "").unwrap();
}

#[test]
fn git_bash_is_found_where_git_for_windows_puts_it_and_nowhere_else() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let none = ShellSources::default();
    assert_eq!(git_bash(&none), None);

    // The folder recorded for all users first, then Program Files, then a
    // folder recorded for this user only (which any of the user's programs
    // can change).
    let recorded = root.join("Tools").join("Git");
    touch(&recorded.join("bin").join("bash.exe"));
    let program_files = root.join("Program Files");
    touch(&program_files.join("Git").join("bin").join("bash.exe"));
    let per_user = root.join("Users").join("me").join("Git");
    touch(&per_user.join("bin").join("bash.exe"));
    let sources = ShellSources {
        install_path: vec![root.join("missing"), recorded.clone()],
        program_files: Some(program_files.clone()),
        user_install_path: vec![per_user.clone()],
        path: None,
    };
    assert_eq!(
        git_bash(&sources),
        Some(recorded.join("bin").join("bash.exe"))
    );
    let sources = ShellSources {
        install_path: Vec::new(),
        program_files: Some(program_files.clone()),
        user_install_path: vec![per_user.clone()],
        path: None,
    };
    assert_eq!(
        git_bash(&sources),
        Some(program_files.join("Git").join("bin").join("bash.exe"))
    );
    let sources = ShellSources {
        install_path: vec![root.join("missing")],
        program_files: Some(root.join("nowhere")),
        user_install_path: vec![per_user.clone()],
        path: None,
    };
    assert_eq!(
        git_bash(&sources),
        Some(per_user.join("bin").join("bash.exe"))
    );

    // On PATH: only a Git for Windows tree's bin\bash.exe, never WSL's
    // launcher in System32, an app alias, or a lone bash.exe.
    let portable = root.join("PortableGit");
    touch(&portable.join("bin").join("bash.exe"));
    touch(&portable.join("usr").join("bin").join("bash.exe"));
    let system32 = root.join("Windows").join("System32");
    touch(&system32.join("bash.exe"));
    let alias = root.join("WindowsApps");
    touch(&alias.join("bash.exe"));
    let lone = root.join("other").join("bin");
    touch(&lone.join("bash.exe"));
    let path = |dirs: &[&Path]| ShellSources {
        install_path: Vec::new(),
        program_files: Some(root.join("nowhere")),
        user_install_path: Vec::new(),
        path: Some(std::env::join_paths(dirs).unwrap()),
    };
    assert_eq!(git_bash(&path(&[&system32, &alias, &lone])), None);
    assert_eq!(
        git_bash(&path(&[&system32, &portable.join("bin")])),
        Some(portable.join("bin").join("bash.exe"))
    );
    // usr\bin on PATH: the launcher in bin, which sets up the tools.
    assert_eq!(
        git_bash(&path(&[&portable.join("usr").join("bin")])),
        Some(portable.join("bin").join("bash.exe"))
    );
}

#[test]
fn the_environment_is_built_from_scratch_without_secrets() {
    let scratch = Path::new("/data/code-scratch/abc");
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".rustup")).unwrap();
    // Every variable the parent has, a provider key among them.
    let parent = |name: &str| -> Option<String> {
        match name {
            "CARGO_HOME" => Some("/opt/cargo".to_string()),
            // Not set: the default folder in the real home is used.
            "RUSTUP_HOME" => None,
            _ => Some(format!("canary-{name}")),
        }
    };
    let env = command_env(scratch, "run-1", parent, Some(home.path()));
    let names: Vec<&str> = env.iter().map(|(name, _)| name.as_str()).collect();
    for (name, value) in &env {
        // Only the system variables programs need come from Shodh's own
        // environment; nothing else is inherited.
        if value.starts_with("canary-") {
            assert!(
                super::super::sidecar::PASSTHROUGH_VARS.contains(&name.as_str()),
                "{name} inherited"
            );
        }
        assert!(
            !name.to_ascii_lowercase().contains("proxy"),
            "{name}={value}"
        );
    }
    // The user's registry and git accounts are not used without a prompt
    // that says so: cargo's token is not one, git has no credential helper.
    for (name, guard) in ACCOUNT_GUARDS {
        let set: Vec<&str> = env
            .iter()
            .filter(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
            .collect();
        assert_eq!(set, [guard], "{name}");
    }
    let guard = |name: &str| {
        ACCOUNT_GUARDS
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| *v)
    };
    assert_eq!(guard("GIT_CONFIG_KEY_0"), Some("credential.helper"));
    assert_eq!(guard("GIT_CONFIG_VALUE_0"), Some(""));
    assert_eq!(guard("GIT_TERMINAL_PROMPT"), Some("0"));
    assert_eq!(
        guard("CARGO_REGISTRY_GLOBAL_CREDENTIAL_PROVIDERS"),
        Some("cargo:token")
    );
    assert!(guard("CARGO_REGISTRY_TOKEN").is_some_and(|token| !token.is_empty()));
    assert!(names.contains(&"PATH"));
    assert!(names.contains(&"SystemRoot"));
    for name in ["HOME", "USERPROFILE", "TEMP", "TMP", "TMPDIR"] {
        let value = &env.iter().find(|(n, _)| n == name).unwrap().1;
        assert_eq!(value, &scratch.display().to_string(), "{name}");
    }
    let value = |name: &str| env.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone());
    // The user's toolchain settings, else the default folders that exist.
    assert_eq!(value("CARGO_HOME").as_deref(), Some("/opt/cargo"));
    assert_eq!(
        value("RUSTUP_HOME"),
        Some(home.path().join(".rustup").display().to_string())
    );
    assert_eq!(value(RUN_ID_ENV).as_deref(), Some("run-1"));
    // A default folder that does not exist is not passed.
    let bare = command_env(scratch, "run-2", |_| None, Some(home.path()));
    assert!(bare.iter().all(|(name, _)| name != "CARGO_HOME"));
    assert!(bare.iter().any(|(name, _)| name == "RUSTUP_HOME"));
}

#[test]
fn scratch_folders_are_per_conversation_and_removed_with_it() {
    let data = tempfile::tempdir().unwrap();
    let a = scratch_dir(data.path(), "conv-1");
    assert_eq!(a, scratch_dir(data.path(), "conv-1"));
    assert_ne!(a, scratch_dir(data.path(), "conv-2"));
    assert!(a.starts_with(data.path().join(SCRATCH_DIR)));
    // Any id gives one plain folder name.
    let odd = scratch_dir(data.path(), "../../x");
    assert_eq!(odd.parent(), a.parent());
    prepare_scratch(&a).unwrap();
    std::fs::write(a.join("file"), "x").unwrap();
    // Prepared again (the next command), its files stay.
    prepare_scratch(&a).unwrap();
    assert!(a.join("file").is_file());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&a).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }
    remove_scratch(data.path(), "conv-1").unwrap();
    assert!(!a.exists());
    remove_scratch(data.path(), "conv-1").unwrap();

    // A file in the folder's place is refused.
    let file = scratch_dir(data.path(), "conv-file");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "x").unwrap();
    assert!(prepare_scratch(&file).is_err());
}

#[cfg(unix)]
#[test]
fn a_scratch_folder_is_a_private_real_folder() {
    use std::os::unix::fs::PermissionsExt;
    let data = tempfile::tempdir().unwrap();
    // Made readable to others since: private again before the next command.
    let dir = scratch_dir(data.path(), "conv-1");
    prepare_scratch(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    prepare_scratch(&dir).unwrap();
    let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o700);
    // A link in its place, to a folder elsewhere, is refused, and the folder
    // it points to is left as it is.
    let elsewhere = data.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::set_permissions(&elsewhere, std::fs::Permissions::from_mode(0o755)).unwrap();
    let linked = scratch_dir(data.path(), "conv-2");
    std::os::unix::fs::symlink(&elsewhere, &linked).unwrap();
    let error = prepare_scratch(&linked).unwrap_err();
    assert!(error.contains("not a plain folder"), "{error}");
    let mode = std::fs::metadata(&elsewhere).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o755);
}

#[cfg(target_os = "macos")]
#[test]
fn the_cpu_time_limit_leaves_room_for_every_core_for_the_whole_time_limit() {
    use super::mac::cpu_limit_secs;
    for (secs, cores) in [(600, 1), (600, 8), (600, 24), (10, 4)] {
        let limit = cpu_limit_secs(secs, cores);
        assert!(limit >= secs * cores, "{secs} s on {cores} cores: {limit}");
        assert!(
            limit <= secs * cores + 60,
            "{secs} s on {cores} cores: {limit}"
        );
    }
    // An unknown core count counts as one; nothing overflows.
    assert!(cpu_limit_secs(600, 0) >= 600);
    assert_eq!(cpu_limit_secs(u64::MAX, 64), u64::MAX);
}

#[cfg(target_os = "macos")]
#[test]
fn the_process_limit_is_the_users_count_plus_the_commands_share() {
    use super::mac::process_limit;
    assert_eq!(process_limit(300, 64, 2_666), 364);
    // Never above the system's own limit for the user.
    assert_eq!(process_limit(2_650, 64, 2_666), 2_666);
    assert_eq!(process_limit(u64::MAX, 64, u64::MAX), u64::MAX);
}

/// A control that counts `on_started` calls and forwards live output.
fn run_control(
    command_id: &str,
) -> (
    RunControl,
    Arc<AtomicUsize>,
    tokio::sync::mpsc::UnboundedReceiver<String>,
) {
    let started = Arc::new(AtomicUsize::new(0));
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let counter = Arc::clone(&started);
    (
        RunControl {
            command_id: command_id.to_string(),
            registration: RunRegistration::new(command_id),
            on_started: Box::new(move || {
                counter.fetch_add(1, Ordering::SeqCst);
            }),
            on_output: Box::new(move |text| {
                let _ = tx.send(text);
            }),
        },
        started,
        rx,
    )
}

fn spec_in(folder: &Path, scratch: &Path, command: &str, secs: u64, memory_mb: u64) -> CommandSpec {
    CommandSpec {
        command: command.to_string(),
        folder: folder.to_path_buf(),
        scratch: scratch.to_path_buf(),
        limits: CommandLimits::for_call(&settings(secs.max(10), memory_mb, 100), Some(secs)),
    }
}

#[tokio::test]
async fn without_a_shell_the_command_is_refused_and_nothing_runs() {
    let folder = tempfile::tempdir().unwrap();
    let scratch = folder.path().join("scratch");
    let (control, started, _output) = run_control("refused");
    let outcome = run_contained(
        spec_in(folder.path(), &scratch, "echo ran > ran.txt", 10, 1024),
        Err(NO_GIT_BASH.to_string()),
        control,
    )
    .await;
    assert_eq!(outcome.state, CommandState::Refused);
    assert_eq!(outcome.note.as_deref(), Some(NO_GIT_BASH));
    assert_eq!(started.load(Ordering::SeqCst), 0);
    assert!(!folder.path().join("ran.txt").exists());
    assert!(!scratch.exists(), "nothing was prepared for it");
    assert!(NO_GIT_BASH.contains("Install Git for Windows"));
    // A run that never started cannot be stopped.
    assert!(!stop("refused"));
}

/// Commands that start real processes, through the runner, with the
/// `sandbox-probe` binary (built by CI before these tests). One at a time:
/// a fork loop next to a heartbeat test would make timings depend on each
/// other.
#[cfg(any(windows, target_os = "macos"))]
mod processes {
    use std::path::PathBuf;
    use std::time::Instant;

    use super::*;

    static ONE_AT_A_TIME: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// The probe, next to this test binary (`target/debug/deps/..`).
    fn probe() -> PathBuf {
        let exe = std::env::current_exe().unwrap();
        let name = format!("sandbox-probe{}", std::env::consts::EXE_SUFFIX);
        let found = exe
            .ancestors()
            .skip(1)
            .take(3)
            .map(|dir| dir.join(&name))
            .find(|path| path.is_file());
        found.unwrap_or_else(|| {
            panic!(
                "{name} is not built next to {}: run `cargo build -p sandbox-probe` first",
                exe.display()
            )
        })
    }

    /// `path` as bash reads it, quoted.
    fn sh(path: &Path) -> String {
        format!("'{}'", path.display().to_string().replace('\\', "/"))
    }

    fn probe_command(args: &str) -> String {
        format!("{} {args}", sh(&probe()))
    }

    fn shell() -> Result<PathBuf, String> {
        let shell = locate_shell();
        assert!(shell.is_ok(), "no shell on this runner: {shell:?}");
        shell
    }

    /// Run the probe directly, outside the runner (the control).
    fn control_run(args: &[&str]) -> std::process::Output {
        std::process::Command::new(probe())
            .args(args)
            .output()
            .unwrap()
    }

    #[cfg(windows)]
    fn alive(pid: u32) -> bool {
        use windows::Win32::Foundation::{CloseHandle, BOOL, STILL_ACTIVE};
        use windows::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        let opened =
            unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, BOOL::from(false), pid) };
        let Ok(process) = opened else {
            return false;
        };
        let mut code = 0u32;
        let read = unsafe { GetExitCodeProcess(process, &mut code) };
        let _ = unsafe { CloseHandle(process) };
        read.is_ok() && code == STILL_ACTIVE.0 as u32
    }

    #[cfg(unix)]
    fn alive(pid: u32) -> bool {
        let pid = libc::pid_t::try_from(pid).unwrap();
        if unsafe { libc::kill(pid, 0) } != 0 {
            return false;
        }
        // A zombie has ended; it only waits to be reaped.
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        let read = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                (&mut info as *mut libc::proc_bsdinfo).cast(),
                size,
            )
        };
        read == size && info.pbi_status != 5
    }

    #[cfg(windows)]
    fn kill(pid: u32) {
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/PID", &pid.to_string()])
            .output();
    }

    #[cfg(unix)]
    fn kill(pid: u32) {
        unsafe {
            libc::kill(libc::pid_t::try_from(pid).unwrap(), libc::SIGKILL);
        }
    }

    /// The pids a probe printed (`pid N` lines).
    fn pids(output: &str) -> Vec<u32> {
        output
            .lines()
            .filter_map(|line| line.trim().strip_prefix("pid "))
            .filter_map(|pid| pid.trim().parse().ok())
            .collect()
    }

    fn beats(file: &Path) -> u64 {
        std::fs::metadata(file).map(|m| m.len()).unwrap_or(0)
    }

    struct Folders {
        _dir: tempfile::TempDir,
        folder: PathBuf,
        scratch: PathBuf,
    }

    fn folders() -> Folders {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("code");
        std::fs::create_dir_all(&folder).unwrap();
        let scratch = dir.path().join("scratch");
        Folders {
            folder,
            scratch,
            _dir: dir,
        }
    }

    /// Provider keys and other secrets in Shodh's own environment, set once
    /// for the whole test process to random canaries.
    fn canaries() -> &'static Vec<(String, String)> {
        static CANARIES: OnceLock<Vec<(String, String)>> = OnceLock::new();
        CANARIES.get_or_init(|| {
            let names = [
                "OPENROUTER_API_KEY",
                "ANTHROPIC_API_KEY",
                "OPENAI_API_KEY",
                "GEMINI_API_KEY",
                "XAI_API_KEY",
                "SHODH_TEST_SECRET",
                "SLACK_BOT_TOKEN",
                "SOME_UNRELATED_SETTING",
            ];
            names
                .iter()
                .map(|name| {
                    let value = format!("canary{}", uuid::Uuid::new_v4().simple());
                    std::env::set_var(name, &value);
                    (name.to_string(), value)
                })
                .collect()
        })
    }

    #[tokio::test]
    async fn the_command_sees_no_key_or_secret_of_shodhs_environment() {
        let _one = ONE_AT_A_TIME.lock().await;
        let canaries = canaries();
        let dirs = folders();
        // Control: the probe sees every canary when Shodh's environment is
        // passed on.
        let direct = String::from_utf8_lossy(&control_run(&["env"]).stdout).into_owned();
        for (name, value) in canaries {
            assert!(direct.contains(value.as_str()), "control misses {name}");
        }
        let (control, started, _output) = run_control("env-run");
        let outcome = run_contained(
            spec_in(&dirs.folder, &dirs.scratch, &probe_command("env"), 30, 1024),
            shell(),
            control,
        )
        .await;
        assert_eq!(outcome.state, CommandState::Done, "{outcome:?}");
        assert_eq!(outcome.exit_code, Some(0), "{outcome:?}");
        assert_eq!(started.load(Ordering::SeqCst), 1);
        for (name, value) in canaries {
            assert!(!outcome.output.contains(value.as_str()), "{name} leaked");
            assert!(
                !outcome
                    .output
                    .lines()
                    .any(|l| l.starts_with(&format!("{name}="))),
                "{name} is set"
            );
        }
        let scratch = dirs.scratch.display().to_string();
        let var = |name: &str| {
            outcome
                .output
                .lines()
                .find_map(|line| line.strip_prefix(&format!("{name}=")))
                .map(str::to_string)
        };
        assert_eq!(var(RUN_ID_ENV).as_deref(), Some("env-run"));
        // The account guards reach the command's own programs.
        for (name, guard) in ACCOUNT_GUARDS {
            assert_eq!(var(name).as_deref(), Some(guard), "{name}");
        }
        // Git Bash hands HOME, TEMP and TMP to Windows programs converted
        // back from its own form: compared as Windows paths.
        #[cfg(windows)]
        {
            let windows = |path: &str| path.replace('/', "\\").to_lowercase();
            assert_eq!(var("USERPROFILE").as_deref(), Some(scratch.as_str()));
            for name in ["HOME", "TEMP", "TMP"] {
                assert_eq!(
                    var(name).map(|v| windows(&v)),
                    Some(windows(&scratch)),
                    "{name}"
                );
            }
        }
        #[cfg(unix)]
        for name in ["HOME", "TMPDIR"] {
            assert_eq!(var(name).as_deref(), Some(scratch.as_str()), "{name}");
        }
        assert!(var("PATH").is_some());
        assert!(dirs.scratch.is_dir());
    }

    #[tokio::test]
    async fn nothing_a_command_starts_outlives_it() {
        let _one = ONE_AT_A_TIME.lock().await;
        let dirs = folders();
        // Control: outside the runner, the detached heartbeat keeps beating
        // after the probe that started it exited.
        let control_file = dirs.folder.join("control-beat.txt");
        // Without pipes: on Windows the heartbeat inherits them, and reading
        // the probe's output to its end would wait for the heartbeat.
        let status = std::process::Command::new(probe())
            .args(["detach", &control_file.display().to_string()])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
        let control_pid: u32 = std::fs::read_to_string(format!("{}.pid", control_file.display()))
            .expect("the control wrote its child's pid")
            .trim()
            .parse()
            .unwrap();
        tokio::time::sleep(Duration::from_millis(600)).await;
        let first = beats(&control_file);
        tokio::time::sleep(Duration::from_millis(600)).await;
        let control_kept_beating = beats(&control_file) > first && first > 0;
        let control_alive = alive(control_pid);
        kill(control_pid);
        assert!(control_kept_beating, "the control's heartbeat stopped");
        assert!(control_alive);

        // Through the runner: the command ends at once, and its detached
        // heartbeat stops within a second of that.
        let file = dirs.folder.join("beat.txt");
        let (control, _started, _output) = run_control("detach-run");
        let outcome = run_contained(
            spec_in(
                &dirs.folder,
                &dirs.scratch,
                &probe_command(&format!("detach {}", sh(&file))),
                30,
                1024,
            ),
            shell(),
            control,
        )
        .await;
        let ended = Instant::now();
        assert_eq!(outcome.state, CommandState::Done, "{outcome:?}");
        assert_eq!(outcome.exit_code, Some(0), "{outcome:?}");
        let pid = *pids(&outcome.output)
            .first()
            .expect("the probe printed its child's pid");
        // Windows: leaving the job is refused outright.
        #[cfg(windows)]
        assert!(
            outcome.output.contains("breakaway refused"),
            "{}",
            outcome.output
        );
        // The process left running was stopped, and the result says so.
        assert!(
            outcome
                .note
                .as_deref()
                .is_some_and(|note| note.contains("still running when it ended")),
            "{outcome:?}"
        );
        tokio::time::sleep_until((ended + Duration::from_secs(1)).into()).await;
        let at_one_second = beats(&file);
        tokio::time::sleep(Duration::from_millis(700)).await;
        assert_eq!(beats(&file), at_one_second, "the heartbeat went on");
        assert!(!alive(pid), "process {pid} outlived its command");
    }

    #[tokio::test]
    async fn a_fork_loop_stops_at_the_process_limit() {
        let _one = ONE_AT_A_TIME.lock().await;
        let dirs = folders();
        let (control, _started, _output) = run_control("fork-run");
        let began = Instant::now();
        let outcome = run_contained(
            spec_in(
                &dirs.folder,
                &dirs.scratch,
                &probe_command("fork-loop 500"),
                60,
                4096,
            ),
            shell(),
            control,
        )
        .await;
        assert!(began.elapsed() < Duration::from_secs(60), "{outcome:?}");
        #[cfg(windows)]
        assert_eq!(outcome.state, CommandState::Done, "{outcome:?}");
        #[cfg(target_os = "macos")]
        assert!(
            matches!(outcome.state, CommandState::Done | CommandState::Killed),
            "{outcome:?}"
        );
        assert!(
            outcome.output.contains("refused after"),
            "{}",
            outcome.output
        );
        let spawned: u32 = outcome
            .output
            .lines()
            .find_map(|line| line.strip_prefix("spawned "))
            .and_then(|n| n.trim().parse().ok())
            .expect("the probe said how many it started");
        let note = outcome.note.clone().unwrap_or_default();
        // Windows: the job refuses past the limit, and says so.
        #[cfg(windows)]
        {
            assert!(spawned < MAX_PROCESSES, "{spawned} processes started");
            assert!(note.contains("process limit"), "{note}");
            assert!(note.contains("still running when it ended"), "{note}");
        }
        // macOS: the per-user backstop bounds the loop (the command's own
        // limit is the watchdog's: `a_command_with_too_many_processes_is_stopped`);
        // a run the watchdog caught over the limit says so.
        #[cfg(target_os = "macos")]
        if outcome.state == CommandState::Killed {
            assert!(note.contains("process limit"), "{note}");
        }
        let left: Vec<u32> = outcome
            .output
            .lines()
            .find_map(|line| line.strip_prefix("pids "))
            .map(|pids| {
                pids.split_whitespace()
                    .filter_map(|p| p.parse().ok())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(left.len() as u32, spawned);
        assert!(left.iter().all(|&pid| !alive(pid)), "a child survived");
    }

    /// macOS keeps a command's process limit with its watchdog: a command
    /// holding more processes than its limit is stopped, with every one of
    /// them.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn a_command_with_too_many_processes_is_stopped() {
        let _one = ONE_AT_A_TIME.lock().await;
        let dirs = folders();
        let (control, _started, _output) = run_control("fork-hold-run");
        let mut spec = spec_in(
            &dirs.folder,
            &dirs.scratch,
            &probe_command("fork-hold 20"),
            60,
            4096,
        );
        spec.limits.max_processes = 8;
        let outcome = run_contained(spec, shell(), control).await;
        assert_eq!(outcome.state, CommandState::Killed, "{outcome:?}");
        let note = outcome.note.clone().unwrap_or_default();
        assert!(note.contains("the process limit (8 at once)"), "{note}");
        assert!(outcome.duration < Duration::from_secs(10), "{outcome:?}");
        let left: Vec<u32> = outcome
            .output
            .lines()
            .find_map(|line| line.strip_prefix("pids "))
            .map(|pids| {
                pids.split_whitespace()
                    .filter_map(|p| p.parse().ok())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(left.len(), 20, "{}", outcome.output);
        assert!(left.iter().all(|&pid| !alive(pid)), "a child survived");
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn every_process_of_a_fork_loop_is_stopped_when_it_ends() {
        let _one = ONE_AT_A_TIME.lock().await;
        let dirs = folders();
        let (control, _started, _output) = run_control("fork-run");
        let outcome = run_contained(
            spec_in(
                &dirs.folder,
                &dirs.scratch,
                &probe_command("fork-loop 40"),
                60,
                4096,
            ),
            shell(),
            control,
        )
        .await;
        assert_eq!(outcome.state, CommandState::Done, "{outcome:?}");
        let left: Vec<u32> = outcome
            .output
            .lines()
            .find_map(|line| line.strip_prefix("pids "))
            .map(|pids| {
                pids.split_whitespace()
                    .filter_map(|p| p.parse().ok())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(left.len(), 40, "{}", outcome.output);
        assert!(left.iter().all(|&pid| !alive(pid)), "a child survived");
    }

    #[tokio::test]
    async fn a_busy_loop_is_stopped_at_its_time_limit() {
        let _one = ONE_AT_A_TIME.lock().await;
        let dirs = folders();
        let (control, _started, _output) = run_control("busy-run");
        let began = Instant::now();
        let outcome = run_contained(
            spec_in(&dirs.folder, &dirs.scratch, &probe_command("busy"), 3, 1024),
            shell(),
            control,
        )
        .await;
        let took = began.elapsed();
        assert_eq!(outcome.state, CommandState::TimedOut, "{outcome:?}");
        assert!(took >= Duration::from_secs(3), "{took:?}");
        // The limit, then at most the drain bound and the process's own end.
        assert!(
            took < Duration::from_secs(3) + DRAIN_GRACE * 2 + Duration::from_secs(2),
            "{took:?}"
        );
        assert!(outcome
            .text_for_model(&CommandLimits::for_call(&settings(10, 1024, 100), Some(3)))
            .contains("time limit (3 seconds)"));
    }

    #[tokio::test]
    async fn memory_beyond_the_limit_is_refused_or_ends_the_command() {
        let _one = ONE_AT_A_TIME.lock().await;
        let dirs = folders();
        // Control: the same allocation succeeds outside the runner.
        let direct = control_run(&["alloc", "1024"]);
        assert!(direct.status.success(), "{direct:?}");
        assert!(String::from_utf8_lossy(&direct.stdout).contains("allocated 1024 MB"));

        let (control, _started, _output) = run_control("alloc-run");
        let outcome = run_contained(
            spec_in(
                &dirs.folder,
                &dirs.scratch,
                &probe_command("alloc 1024 30"),
                60,
                256,
            ),
            shell(),
            control,
        )
        .await;
        assert!(!outcome.output.contains("allocated 1024 MB"), "{outcome:?}");
        // Windows: the job refuses the allocation, and the result says so.
        #[cfg(windows)]
        {
            assert_eq!(outcome.state, CommandState::Done, "{outcome:?}");
            assert_eq!(outcome.exit_code, Some(3), "{outcome:?}");
            assert!(outcome.output.contains("allocation failed"), "{outcome:?}");
            let note = outcome.note.clone().unwrap_or_default();
            assert!(note.contains("memory limit (256 MB"), "{note}");
        }
        // macOS: the watchdog ends the command.
        #[cfg(target_os = "macos")]
        {
            assert_eq!(outcome.state, CommandState::Killed, "{outcome:?}");
            assert!(
                outcome
                    .note
                    .as_deref()
                    .unwrap_or_default()
                    .contains("memory limit (256 MB"),
                "{outcome:?}"
            );
            assert!(outcome.duration < Duration::from_secs(30), "{outcome:?}");
        }
    }

    #[tokio::test]
    async fn stop_ends_the_command_and_everything_it_started() {
        let _one = ONE_AT_A_TIME.lock().await;
        let dirs = folders();
        let (control, started, mut output) = run_control("stop-run");
        let run = tokio::spawn(run_contained(
            spec_in(
                &dirs.folder,
                &dirs.scratch,
                &probe_command("children 3"),
                120,
                1024,
            ),
            shell(),
            control,
        ));
        // Wait until the three children are running.
        let mut seen = String::new();
        let deadline = Instant::now() + Duration::from_secs(30);
        while pids(&seen).len() < 3 {
            let text = tokio::time::timeout_at(deadline.into(), output.recv())
                .await
                .expect("the children started within 30 s")
                .expect("output open");
            seen.push_str(&text);
        }
        assert_eq!(started.load(Ordering::SeqCst), 1);
        let children = pids(&seen);
        assert!(children.iter().all(|&pid| alive(pid)));
        let stopped_at = Instant::now();
        assert!(stop("stop-run"));
        let outcome = tokio::time::timeout(Duration::from_secs(10), run)
            .await
            .expect("the run ended within 10 s of Stop")
            .unwrap();
        assert!(stopped_at.elapsed() < Duration::from_secs(5));
        assert_eq!(outcome.state, CommandState::Stopped, "{outcome:?}");
        assert!(
            children.iter().all(|&pid| !alive(pid)),
            "a child outlived Stop"
        );
        // The run is gone: nothing more to stop.
        assert!(!stop("stop-run"));
    }

    #[tokio::test]
    async fn ordinary_commands_work_and_reach_bash_unchanged() {
        let _one = ONE_AT_A_TIME.lock().await;
        let dirs = folders();
        std::fs::write(dirs.folder.join("x.txt"), "x").unwrap();
        let python = if cfg!(windows) { "python" } else { "python3" };
        for command in [
            format!("ls | grep x && {python} -c 'import sys; print(sys.version_info[0])'"),
            "git init -q && git status".to_string(),
            "cargo --version".to_string(),
            // No credential helper, whatever the system's git settings name
            // (Git for Windows: the Credential Manager; macOS: the keychain).
            "test -z \"$(git config --get credential.helper)\"".to_string(),
        ] {
            let (control, _started, _output) = run_control("usual-run");
            let outcome = run_contained(
                spec_in(&dirs.folder, &dirs.scratch, &command, 120, 4096),
                shell(),
                control,
            )
            .await;
            assert_eq!(outcome.state, CommandState::Done, "{command}: {outcome:?}");
            assert_eq!(outcome.exit_code, Some(0), "{command}: {outcome:?}");
        }
        // Quotes, backslashes, `$`, `!`, `%`, globs, non-ASCII and a line
        // break reach bash byte for byte.
        let hostile = "a \"b\" 'c' \\d $HOME !x %PATH% ls* é✓ C:\\x\\y\nsecond line";
        let command = format!("cat <<'SHODH_END'\n{hostile}\nSHODH_END");
        let (control, _started, _output) = run_control("quote-run");
        let outcome = run_contained(
            spec_in(&dirs.folder, &dirs.scratch, &command, 30, 1024),
            shell(),
            control,
        )
        .await;
        assert_eq!(outcome.exit_code, Some(0), "{outcome:?}");
        assert_eq!(outcome.output, format!("{hostile}\n"));
        // A command starting like a drive reaches bash too.
        let (control, _started, _output) = run_control("drive-run");
        let outcome = run_contained(
            spec_in(&dirs.folder, &dirs.scratch, "c:; echo after", 30, 1024),
            shell(),
            control,
        )
        .await;
        assert!(outcome.output.contains("after"), "{outcome:?}");
    }
}
