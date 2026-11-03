use super::*;

const NOW: &str = "2026-11-03T10:00:00Z";
const LATER: &str = "2026-11-03T11:00:00Z";

struct Setup {
    _dir: tempfile::TempDir,
    documents: PathBuf,
    root: PathBuf,
    store: CreationStore,
}

fn setup() -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let documents = dir.path().join("Documents");
    let data = dir.path().join("data");
    fs::create_dir_all(&documents).unwrap();
    let root = creations_root(&documents).unwrap();
    Setup {
        store: CreationStore::in_dir(&data),
        _dir: dir,
        documents,
        root,
    }
}

fn write(folder: &Path, relative: &str, text: &str) {
    let path = folder.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

#[test]
fn a_request_names_its_folder_safely() {
    assert_eq!(
        slug_of("Make me a pomodoro timer app"),
        "pomodoro-timer-app"
    );
    assert_eq!(slug_of("Build a game: Snake!"), "game-snake");
    assert_eq!(slug_of("!!!"), "creation");
    assert_eq!(slug_of("CON"), "con-creation");
    assert_eq!(slug_of("make a"), "make-a");
    let long = slug_of(&"word ".repeat(40));
    assert!(long.len() <= MAX_SLUG_CHARS, "{long}");
    assert!(!long.ends_with('-') && !long.ends_with('.'));
    // Only ASCII letters and digits reach the name.
    assert_eq!(slug_of("café ../../etc\\passwd"), "caf-etc-passwd");
    assert_eq!(title_of("\n  A  timer\nwith more\n"), "A timer");
    assert_eq!(title_of("   "), "Untitled creation");
}

#[test]
fn ids_are_exactly_the_store_form() {
    let id = new_id();
    assert!(valid_id(&id), "{id}");
    for bad in [
        "",
        "cr_",
        "../cr_0123456789abcdef0123456789abcdef",
        "cr_0123456789ABCDEF0123456789abcdef",
        "cr_0123456789abcdef0123456789abcde/",
        "xx_0123456789abcdef0123456789abcdef",
    ] {
        assert!(!valid_id(bad), "{bad}");
    }
}

#[test]
fn a_conversation_gets_one_fresh_folder_and_never_an_existing_one() {
    let s = setup();
    // A folder already there with the name is never taken over.
    fs::create_dir(s.root.join("pomodoro-timer")).unwrap();
    write(&s.root.join("pomodoro-timer"), "mine.txt", "the user's");
    let first = s
        .store
        .prepare(&s.root, "conv-a", "a pomodoro timer", NOW)
        .unwrap();
    assert_eq!(
        Path::new(&first.folder),
        s.root.join("pomodoro-timer-2").as_path()
    );
    assert!(fs::read_dir(&first.folder).unwrap().next().is_none());
    assert_eq!(first.status, CreationStatus::Building);
    assert_eq!(first.title, "a pomodoro timer");
    assert!(valid_id(&first.id));
    // The same conversation keeps its folder; another gets its own.
    let again = s
        .store
        .prepare(&s.root, "conv-a", "something else", LATER)
        .unwrap();
    assert_eq!(again, first);
    let other = s
        .store
        .prepare(&s.root, "conv-b", "a pomodoro timer", NOW)
        .unwrap();
    assert_eq!(
        Path::new(&other.folder),
        s.root.join("pomodoro-timer-3").as_path()
    );
    assert_eq!(
        s.store.for_conversation("conv-b").unwrap().unwrap().id,
        other.id
    );
    assert_eq!(
        fs::read_to_string(s.root.join("pomodoro-timer").join("mine.txt")).unwrap(),
        "the user's"
    );
}

#[cfg(unix)]
#[test]
fn a_creations_folder_that_is_a_link_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let documents = dir.path().join("Documents");
    let elsewhere = dir.path().join("elsewhere");
    fs::create_dir_all(&documents).unwrap();
    fs::create_dir_all(&elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, documents.join(CREATIONS_FOLDER)).unwrap();
    let error = creations_root(&documents).unwrap_err();
    assert!(error.contains("link"), "{error}");
}

#[test]
fn each_finished_answer_saves_a_version_only_when_something_changed() {
    let s = setup();
    let creation = s.store.prepare(&s.root, "conv", "a site", NOW).unwrap();
    let folder = PathBuf::from(&creation.folder);
    write(&folder, "index.html", "<h1>Hi</h1>\n");
    write(&folder, "src/app.js", "console.log(1);\n");
    // Dependencies, caches and git's folder are not part of a version.
    write(&folder, "node_modules/react/index.js", "x");
    write(&folder, ".git/config", "[core]");
    write(&folder, "target/debug/app", "bin");
    let v1 = s
        .store
        .snapshot(&s.root, "conv", Some("run-1"), NOW)
        .unwrap()
        .unwrap();
    assert_eq!((v1.number, v1.files, v1.left_out), (1, 2, 0));
    assert_eq!(v1.run_id.as_deref(), Some("run-1"));
    let paths: Vec<String> = s
        .store
        .files(&s.root, &creation.id, Some(1))
        .unwrap()
        .into_iter()
        .map(|f| f.path)
        .collect();
    assert_eq!(paths, ["index.html", "src/app.js"]);
    let record = s.store.get(&creation.id).unwrap().unwrap();
    assert_eq!(record.status, CreationStatus::Ready);
    // Nothing changed: no new version.
    assert_eq!(
        s.store.snapshot(&s.root, "conv", None, LATER).unwrap(),
        None
    );
    // A change, a new file and a removed file: version 2, and version 1 stays.
    write(&folder, "index.html", "<h1>Hello</h1>\n");
    write(&folder, "style.css", "h1 { color: red }\n");
    fs::remove_file(folder.join("src/app.js")).unwrap();
    let v2 = s
        .store
        .snapshot(&s.root, "conv", Some("run-2"), LATER)
        .unwrap()
        .unwrap();
    assert_eq!(v2.number, 2);
    let old = s
        .store
        .file(&s.root, &creation.id, Some(1), "index.html")
        .unwrap();
    assert_eq!(old.text.as_deref(), Some("<h1>Hi</h1>\n"));
    let changes = s.store.diff(&creation.id, 1, 2).unwrap();
    let summary: Vec<(&str, &str)> = changes
        .iter()
        .map(|c| (c.path.as_str(), c.status))
        .collect();
    assert_eq!(
        summary,
        [
            ("index.html", "changed"),
            ("src/app.js", "removed"),
            ("style.css", "added")
        ]
    );
    let lines: Vec<(&str, &str)> = changes[0].hunks[0]
        .lines
        .iter()
        .map(|l| (l.kind, l.text.as_str()))
        .collect();
    assert_eq!(
        lines,
        [("removed", "<h1>Hi</h1>"), ("added", "<h1>Hello</h1>")]
    );
    // From nothing, everything is added.
    assert!(s
        .store
        .diff(&creation.id, 0, 1)
        .unwrap()
        .iter()
        .all(|c| c.status == "added"));
    assert!(s.store.diff(&creation.id, 1, 7).is_err());
}

#[test]
fn binary_and_large_files_are_shown_as_such() {
    let s = setup();
    let creation = s.store.prepare(&s.root, "conv", "an image", NOW).unwrap();
    let folder = PathBuf::from(&creation.folder);
    fs::write(folder.join("logo.png"), [0x89, b'P', b'N', b'G', 0, 1, 2]).unwrap();
    let big = "line\n".repeat(MAX_VIEW_BYTES / 5 + 10);
    write(&folder, "big.txt", &big);
    s.store.snapshot(&s.root, "conv", None, NOW).unwrap();
    let logo = s
        .store
        .file(&s.root, &creation.id, None, "logo.png")
        .unwrap();
    assert!(logo.binary && logo.text.is_none());
    let shown = s
        .store
        .file(&s.root, &creation.id, Some(1), "big.txt")
        .unwrap();
    assert!(shown.truncated && !shown.binary);
    assert_eq!(shown.text.unwrap().len(), MAX_VIEW_BYTES);
    let changes = s.store.diff(&creation.id, 0, 1).unwrap();
    let logo = changes.iter().find(|c| c.path == "logo.png").unwrap();
    assert!(logo.binary && !logo.too_large && logo.hunks.is_empty());
    let big = changes.iter().find(|c| c.path == "big.txt").unwrap();
    assert!(big.too_large && !big.binary && big.hunks.is_empty());
}

#[test]
fn reads_stay_inside_the_folder() {
    let s = setup();
    let creation = s.store.prepare(&s.root, "conv", "a site", NOW).unwrap();
    let folder = PathBuf::from(&creation.folder);
    write(&folder, "index.html", "ok");
    write(&s.documents, "secret.txt", "secret");
    for relative in [
        "../secret.txt",
        "../../Documents/secret.txt",
        "/etc/passwd",
        "C:\\Windows\\win.ini",
        "src\\..\\..\\secret.txt",
        "",
        "./index.html",
        "node_modules/x.js",
    ] {
        assert!(
            s.store.file(&s.root, &creation.id, None, relative).is_err(),
            "{relative:?}"
        );
    }
    assert!(s
        .store
        .file(
            &s.root,
            "cr_0123456789abcdef0123456789abcdef",
            None,
            "index.html"
        )
        .is_err());
    assert!(s.store.file(&s.root, "../x", None, "index.html").is_err());
    assert_eq!(
        s.store
            .file(&s.root, &creation.id, None, "index.html")
            .unwrap()
            .text
            .as_deref(),
        Some("ok")
    );
}

#[cfg(unix)]
#[test]
fn links_in_a_creation_folder_are_never_followed() {
    let s = setup();
    let creation = s.store.prepare(&s.root, "conv", "a site", NOW).unwrap();
    let folder = PathBuf::from(&creation.folder);
    write(&s.documents, "secret.txt", "secret");
    fs::create_dir_all(s.documents.join("private")).unwrap();
    write(&s.documents.join("private"), "key.txt", "key");
    std::os::unix::fs::symlink(s.documents.join("secret.txt"), folder.join("leak.txt")).unwrap();
    std::os::unix::fs::symlink(s.documents.join("private"), folder.join("dir")).unwrap();
    write(&folder, "index.html", "ok");
    // Not listed, not copied into a version, not readable.
    let live: Vec<String> = s
        .store
        .files(&s.root, &creation.id, None)
        .unwrap()
        .into_iter()
        .map(|f| f.path)
        .collect();
    assert_eq!(live, ["index.html"]);
    s.store.snapshot(&s.root, "conv", None, NOW).unwrap();
    assert_eq!(
        s.store.files(&s.root, &creation.id, Some(1)).unwrap().len(),
        1
    );
    for relative in ["leak.txt", "dir/key.txt"] {
        assert!(
            s.store.file(&s.root, &creation.id, None, relative).is_err(),
            "{relative}"
        );
    }
    // A hard link gives an outside file a name in the folder: never read either.
    fs::hard_link(s.documents.join("secret.txt"), folder.join("copy.txt")).unwrap();
    assert!(s
        .store
        .file(&s.root, &creation.id, None, "copy.txt")
        .is_err());
    s.store.snapshot(&s.root, "conv", None, LATER).unwrap();
    let kept: Vec<String> = s
        .store
        .files(
            &s.root,
            &creation.id,
            s.store.get(&creation.id).unwrap().unwrap().latest_version(),
        )
        .unwrap()
        .into_iter()
        .map(|f| f.path)
        .collect();
    assert!(!kept.contains(&"copy.txt".to_string()), "{kept:?}");
    // A creation folder swapped for a link is left alone.
    fs::remove_dir_all(&folder).unwrap();
    std::os::unix::fs::symlink(s.documents.join("private"), &folder).unwrap();
    assert!(s.store.files(&s.root, &creation.id, None).is_err());
    assert!(s.store.delete(&s.root, &creation.id).is_err());
    assert!(s.documents.join("private").join("key.txt").is_file());
}

#[test]
fn delete_removes_only_a_folder_still_in_shodh_creations() {
    let s = setup();
    let creation = s.store.prepare(&s.root, "conv", "a site", NOW).unwrap();
    let folder = PathBuf::from(&creation.folder);
    write(&folder, "index.html", "ok");
    s.store.snapshot(&s.root, "conv", None, NOW).unwrap();
    // A record pointing elsewhere (moved, or edited by hand) is refused.
    let elsewhere = s.documents.join("Work");
    write(&elsewhere, "report.docx", "work");
    let id = creation.id.clone();
    let moved = elsewhere.display().to_string();
    s.store
        .update(|creations| {
            creations.iter_mut().for_each(|c| c.folder = moved.clone());
            Ok(())
        })
        .unwrap();
    assert!(s.store.delete(&s.root, &id).is_err());
    assert!(elsewhere.join("report.docx").is_file());
    let original = folder.display().to_string();
    s.store
        .update(|creations| {
            creations
                .iter_mut()
                .for_each(|c| c.folder = original.clone());
            Ok(())
        })
        .unwrap();
    let removed = s.store.delete(&s.root, &id).unwrap();
    assert_eq!(removed.id, id);
    assert!(!folder.exists());
    assert!(s.root.is_dir());
    assert!(s.store.get(&id).unwrap().is_none());
    assert!(!s.store.versions_dir(&id).exists());
}

#[test]
fn keep_and_finish_are_recorded() {
    let s = setup();
    let creation = s.store.prepare(&s.root, "conv", "a game", NOW).unwrap();
    s.store
        .record_finish(
            "conv",
            "A snake game",
            vec!["Canvas, no libraries".into()],
            LATER,
        )
        .unwrap();
    let kept = s.store.keep(&creation.id, LATER).unwrap();
    assert_eq!(kept.status, CreationStatus::Kept);
    assert_eq!(kept.summary.as_deref(), Some("A snake game"));
    assert_eq!(kept.choices, ["Canvas, no libraries"]);
    assert_eq!(kept.updated_at, LATER);
}

#[test]
fn an_export_is_a_zip_of_a_version_and_never_overwrites() {
    let s = setup();
    let creation = s.store.prepare(&s.root, "conv", "a site", NOW).unwrap();
    let folder = PathBuf::from(&creation.folder);
    let exports = s.documents.join("Shodh exports");
    assert!(s.store.export(&creation.id, None, &exports).is_err());
    write(&folder, "index.html", "<p>one</p>");
    write(&folder, "js/app.js", "1");
    s.store.snapshot(&s.root, "conv", None, NOW).unwrap();
    let first = s.store.export(&creation.id, None, &exports).unwrap();
    let second = s.store.export(&creation.id, Some(1), &exports).unwrap();
    assert_ne!(first, second);
    let mut zip = zip::ZipArchive::new(fs::File::open(&first).unwrap()).unwrap();
    let mut names: Vec<String> = (0..zip.len())
        .map(|i| zip.by_index(i).unwrap().name().to_string())
        .collect();
    names.sort();
    assert_eq!(names, ["index.html", "js/app.js"]);
    let mut text = String::new();
    zip.by_name("index.html")
        .unwrap()
        .read_to_string(&mut text)
        .unwrap();
    assert_eq!(text, "<p>one</p>");
}

/// The race the open-handle check closes: a folder of the path is swapped for
/// a link to elsewhere after the path was checked and before the file opens.
#[cfg(unix)]
#[test]
fn a_folder_swapped_for_a_link_between_the_check_and_the_open_reads_nothing() {
    let s = setup();
    let creation = s.store.prepare(&s.root, "conv", "a site", NOW).unwrap();
    let folder = PathBuf::from(&creation.folder);
    write(&folder, "src/page.html", "mine");
    let outside = s.documents.join("private");
    write(&outside, "page.html", "secret");
    let page = Path::new("src/page.html");
    let (bytes, _) = read_inside_with(&folder, page, 1024, || {}).unwrap();
    assert_eq!(bytes, b"mine");
    let swapped = read_inside_with(&folder, page, 1024, || {
        fs::remove_dir_all(folder.join("src")).unwrap();
        std::os::unix::fs::symlink(&outside, folder.join("src")).unwrap();
    });
    let error = swapped.expect_err("the swapped path must not be read");
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied, "{error}");
}

/// The same race on Windows, with a junction (made without privileges).
#[cfg(windows)]
#[test]
fn a_folder_swapped_for_a_junction_between_the_check_and_the_open_reads_nothing() {
    let s = setup();
    let creation = s.store.prepare(&s.root, "conv", "a site", NOW).unwrap();
    let folder = PathBuf::from(&creation.folder);
    write(&folder, "src/page.html", "mine");
    let outside = s.documents.join("private");
    write(&outside, "page.html", "secret");
    let page = Path::new("src/page.html");
    let swapped = read_inside_with(&folder, page, 1024, || {
        fs::remove_dir_all(folder.join("src")).unwrap();
        let made = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(folder.join("src"))
            .arg(&outside)
            .output()
            .unwrap();
        assert!(made.status.success(), "{made:?}");
    });
    let error = swapped.expect_err("the swapped path must not be read");
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied, "{error}");
    // And a junction already there is refused by the path check too.
    assert!(s
        .store
        .file(&s.root, &creation.id, None, "src/page.html")
        .is_err());
}

#[test]
fn a_hard_linked_file_is_never_read() {
    let s = setup();
    let creation = s.store.prepare(&s.root, "conv", "a site", NOW).unwrap();
    let folder = PathBuf::from(&creation.folder);
    write(&s.documents, "secret.txt", "secret");
    fs::hard_link(s.documents.join("secret.txt"), folder.join("copy.txt")).unwrap();
    write(&folder, "index.html", "ok");
    assert!(s
        .store
        .file(&s.root, &creation.id, None, "copy.txt")
        .is_err());
    s.store.snapshot(&s.root, "conv", None, NOW).unwrap();
    let kept: Vec<String> = s
        .store
        .files(&s.root, &creation.id, Some(1))
        .unwrap()
        .into_iter()
        .map(|f| f.path)
        .collect();
    assert_eq!(kept, ["index.html"]);
}

#[test]
fn a_folder_made_later_under_the_same_name_is_never_taken_for_an_old_creation() {
    let s = setup();
    let old = s.store.prepare(&s.root, "conv-a", "a site", NOW).unwrap();
    assert!(old.folder_id.is_some());
    // Its folder is removed outside the app; a new creation gets the same name.
    fs::remove_dir_all(&old.folder).unwrap();
    let new = s.store.prepare(&s.root, "conv-b", "a site", LATER).unwrap();
    assert_eq!(new.folder, old.folder);
    write(Path::new(&new.folder), "index.html", "new");
    // The old record no longer reaches the folder, and deleting it leaves the
    // new creation's folder alone.
    assert!(s.store.files(&s.root, &old.id, None).is_err());
    assert!(s.store.snapshot(&s.root, "conv-a", None, LATER).is_err());
    s.store.delete(&s.root, &old.id).unwrap();
    assert!(s.store.get(&old.id).unwrap().is_none());
    assert_eq!(
        fs::read_to_string(Path::new(&new.folder).join("index.html")).unwrap(),
        "new"
    );
    assert_eq!(s.store.files(&s.root, &new.id, None).unwrap().len(), 1);
}
