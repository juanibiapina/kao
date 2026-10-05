mod support;
use sha2::{Digest, Sha256};
use support::Repository;
#[test]
fn captures_a_text_edit_without_redirecting_the_command() {
    let repo = Repository::new("duck pond");
    repo.write("duck.txt", "quack\n");
    repo.commit("Add duck");

    let (status, stdout, stderr, capture) = repo.capture(&[
        "run",
        "--",
        "bash",
        "-c",
        "printf 'honk\\n' > duck.txt; printf 'hello\\n'; printf 'warning\\n' >&2",
    ]);

    assert!(status.success(), "wrapped command should succeed");
    assert_eq!(
        repo.read("duck.txt"),
        "honk\n",
        "Kao must execute the command in the original cwd"
    );
    assert_eq!(
        stdout, "hello\n",
        "command stdout must pass through unchanged"
    );
    assert_eq!(
        stderr, "warning\n",
        "command stderr must pass through unchanged"
    );

    let (files, result) = support::read_capture(&capture);
    assert_eq!(result["format_version"], 1);
    assert!(!result["operation_id"].as_str().unwrap().is_empty());
    assert_eq!(
        result["repository_root"].as_str().unwrap(),
        repo.canonical_path().to_str().unwrap()
    );
    assert_eq!(
        result["cwd"].as_str().unwrap(),
        repo.canonical_path().to_str().unwrap()
    );
    assert_eq!(result["command"]["exit_code"], 0);
    assert_eq!(result["capture"]["complete"], true);
    let changes = result["files"].as_array().unwrap();
    assert_eq!(changes.len(), 1);
    let change = &changes[0];
    assert_eq!(change["path"], "duck.txt");
    assert_eq!(
        change["before_sha256"],
        format!("{:x}", Sha256::digest(b"quack\n"))
    );
    assert_eq!(
        change["after_sha256"],
        format!("{:x}", Sha256::digest(b"honk\n"))
    );
    let blob_path = change["after_blob"].as_str().unwrap();
    assert!(blob_path.starts_with("blobs/"));
    let blob = files
        .get(blob_path)
        .expect("modified text must have an after blob");
    assert_eq!(blob, b"honk\n");
    assert_eq!(
        files.len(),
        3,
        "capture should contain only patch, manifest, and after blob"
    );

    let patch = files
        .get("changes.patch")
        .expect("capture must contain changes.patch");
    let displayed_patch = std::str::from_utf8(patch).unwrap();
    assert!(displayed_patch.contains("diff --git a/duck.txt b/duck.txt"));
    assert!(displayed_patch.contains("\n-quack\n+honk\n"));
    repo.write("duck.txt", std::str::from_utf8(blob).unwrap());
    repo.reverse_patch(patch);
    assert_eq!(
        repo.read("duck.txt"),
        "quack\n",
        "reverse patch must reconstruct the exact before bytes"
    );
    assert_eq!(
        repo.status(),
        "",
        "reverse patch must restore the committed tree"
    );
}
#[test]
fn captures_changes_when_the_command_fails() {
    let repo = Repository::new("duck pond");
    repo.write("duck.txt", "quack\n");
    repo.commit("Add duck");

    let (status, stdout, stderr, capture) = repo.capture(&[
        "run",
        "--",
        "bash",
        "-c",
        "printf 'honk\\n' > duck.txt; exit 7",
    ]);

    assert_eq!(
        status.code(),
        Some(7),
        "Kao must preserve the command exit code"
    );
    assert_eq!(stdout, "");
    assert_eq!(stderr, "", "command failure is not a capture error");
    assert_eq!(repo.read("duck.txt"), "honk\n");

    let (files, result) = support::read_capture(&capture);
    assert_eq!(result["command"]["exit_code"], 7);
    assert_eq!(result["capture"]["complete"], true);
    assert!(result["capture"]["error"].is_null());
    let changes = result["files"].as_array().unwrap();
    assert_eq!(changes.len(), 1);
    let change = &changes[0];
    assert_eq!(change["path"], "duck.txt");
    assert_eq!(
        change["before_sha256"],
        format!("{:x}", Sha256::digest(b"quack\n"))
    );
    assert_eq!(
        change["after_sha256"],
        format!("{:x}", Sha256::digest(b"honk\n"))
    );
    let blob = files
        .get(change["after_blob"].as_str().unwrap())
        .expect("modified text must have an after blob");
    assert_eq!(blob, b"honk\n");
    repo.write("duck.txt", std::str::from_utf8(blob).unwrap());
    repo.reverse_patch(
        files
            .get("changes.patch")
            .expect("capture must contain changes.patch"),
    );
    assert_eq!(repo.read("duck.txt"), "quack\n");
    assert_eq!(repo.status(), "");
}
#[test]
fn concurrent_commands_wait_for_the_repository_lock() {
    let repo = Repository::new("duck pond");
    repo.write("duck.txt", "quack\n");
    repo.commit("Add duck");

    let mut first = repo.spawn_capture(
        "first",
        &[
            "run",
            "--",
            "bash",
            "-c",
            "printf 'first\\n' > duck.txt; printf 'first started\\n'; read -r release",
        ],
    );
    first.wait_for_output("first started\n");
    let mut second = repo.spawn_capture(
        "second",
        &[
            "run",
            "--",
            "bash",
            "-c",
            "printf 'second started\\n'; printf 'second\\n' >> duck.txt",
        ],
    );
    second.assert_waiting("second started\n");
    assert_eq!(repo.read("duck.txt"), "first\n");

    first.release();
    let (first_status, _, first_stderr, first_capture) = first.finish();
    let (second_status, _, second_stderr, second_capture) = second.finish();
    assert!(first_status.success());
    assert!(second_status.success());
    assert_eq!(first_stderr, "");
    assert_eq!(second_stderr, "");
    assert_eq!(repo.read("duck.txt"), "first\nsecond\n");

    let (first_files, first_result) = support::read_capture(&first_capture);
    let (second_files, second_result) = support::read_capture(&second_capture);
    for result in [&first_result, &second_result] {
        assert_eq!(result["capture"]["complete"], true);
        assert_eq!(result["command"]["exit_code"], 0);
        assert_eq!(result["files"].as_array().unwrap().len(), 1);
        assert_eq!(result["files"][0]["path"], "duck.txt");
    }
    let first_change = &first_result["files"][0];
    let second_change = &second_result["files"][0];
    assert_eq!(
        first_change["before_sha256"],
        format!("{:x}", Sha256::digest(b"quack\n"))
    );
    assert_eq!(
        first_change["after_sha256"],
        format!("{:x}", Sha256::digest(b"first\n"))
    );
    assert_eq!(
        second_change["before_sha256"], first_change["after_sha256"],
        "second snapshot must be taken after acquiring the lock"
    );
    assert_eq!(
        second_change["after_sha256"],
        format!("{:x}", Sha256::digest(b"first\nsecond\n"))
    );
    assert_eq!(
        first_files[first_change["after_blob"].as_str().unwrap()],
        b"first\n"
    );
    assert_eq!(
        second_files[second_change["after_blob"].as_str().unwrap()],
        b"first\nsecond\n"
    );
    repo.reverse_patch(&second_files["changes.patch"]);
    assert_eq!(repo.read("duck.txt"), "first\n");
    repo.reverse_patch(&first_files["changes.patch"]);
    assert_eq!(repo.read("duck.txt"), "quack\n");
    assert_eq!(repo.status(), "");
}
#[test]
fn preserves_crlf_and_final_newlines_in_text_captures() {
    for (before, after) in [
        ("quack\r\n", "honk\r\n"),
        ("quack\r\n", "honk"),
        ("quack", "honk\n"),
        ("quack\n", "honk"),
    ] {
        let repo = Repository::new("duck pond");
        repo.write("duck.txt", before);
        repo.commit("Add duck");
        let (status, _, stderr, capture) = repo.capture(&[
            "run",
            "--",
            "bash",
            "-c",
            "printf '%s' \"$1\" > duck.txt",
            "edit",
            after,
        ]);
        assert!(status.success(), "{stderr}");
        let (files, result) = support::read_capture(&capture);
        assert_eq!(result["capture"]["complete"], true);
        assert_eq!(result["files"].as_array().unwrap().len(), 1);
        let change = &result["files"][0];
        assert_eq!(change["path"], "duck.txt");
        assert_eq!(
            change["before_sha256"],
            format!("{:x}", Sha256::digest(before.as_bytes()))
        );
        assert_eq!(
            change["after_sha256"],
            format!("{:x}", Sha256::digest(after.as_bytes()))
        );
        assert_eq!(
            files[change["after_blob"].as_str().unwrap()],
            after.as_bytes()
        );
        assert_eq!(repo.read("duck.txt"), after);
        repo.reverse_patch(&files["changes.patch"]);
        assert_eq!(repo.read("duck.txt"), before);
        assert_eq!(repo.status(), "");
    }
}
#[test]
fn captures_additions_deletions_and_replacements_from_dirty_working_bytes() {
    let repo = Repository::new("duck pond");
    repo.write("duck.txt", "committed\n");
    repo.write("deleted.txt", "gone\r\nwithout final newline");
    repo.write(".gitignore", "ignored/\n");
    repo.commit("Add starting files");
    repo.write("duck.txt", "dirty before\n");
    let before_status = repo.status();

    let (status, _, stderr, capture) = repo.capture(&[
        "run", "--", "bash", "-c",
        "rm deleted.txt; mkdir nested ignored; printf 'created\\r\\nlast line' > nested/created.txt; printf 'replacement\\n' > replacement.tmp; mv replacement.tmp duck.txt; printf 'cache\\n' > ignored/cache",
    ]);
    assert!(status.success(), "{stderr}");
    let (files, result) = support::read_capture(&capture);
    assert_eq!(result["capture"]["complete"], true);
    let changes = result["files"].as_array().unwrap();
    assert_eq!(changes.len(), 3);
    for (path, before, after) in [
        ("duck.txt", Some("dirty before\n"), Some("replacement\n")),
        ("deleted.txt", Some("gone\r\nwithout final newline"), None),
        ("nested/created.txt", None, Some("created\r\nlast line")),
    ] {
        let change = changes
            .iter()
            .find(|change| change["path"] == path)
            .unwrap();
        let expected_before = before
            .map(|text| serde_json::json!(format!("{:x}", Sha256::digest(text.as_bytes()))))
            .unwrap_or(serde_json::Value::Null);
        let expected_after = after
            .map(|text| serde_json::json!(format!("{:x}", Sha256::digest(text.as_bytes()))))
            .unwrap_or(serde_json::Value::Null);
        assert_eq!(change["before_sha256"], expected_before);
        assert_eq!(change["after_sha256"], expected_after);
        if before.is_none() || after.is_none() {
            assert!(
                change["after_blob"].is_null(),
                "patch must supply complete created/deleted text contents"
            );
        }
    }
    assert_eq!(repo.read("duck.txt"), "replacement\n");
    assert_eq!(repo.read("nested/created.txt"), "created\r\nlast line");
    assert!(!repo.canonical_path().join("deleted.txt").exists());
    assert_eq!(repo.read("ignored/cache"), "cache\n");
    repo.reverse_patch(&files["changes.patch"]);
    assert_eq!(repo.read("duck.txt"), "dirty before\n");
    assert_eq!(repo.read("deleted.txt"), "gone\r\nwithout final newline");
    assert!(!repo.canonical_path().join("nested/created.txt").exists());
    assert_eq!(repo.status(), before_status);
}
#[test]
fn captures_reversible_binary_and_mode_changes() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let repo = Repository::new("duck pond");
    let root = repo.canonical_path();
    fs::write(root.join("binary.dat"), b"\0old binary\xff\n").unwrap();
    fs::write(root.join("deleted.dat"), b"\0deleted\xfe").unwrap();
    repo.write("script.sh", "#!/bin/sh\nexit 0\n");
    fs::set_permissions(root.join("script.sh"), fs::Permissions::from_mode(0o644)).unwrap();
    repo.commit("Add binary files and script");

    let (status, _, stderr, capture) = repo.capture(&[
        "run", "--", "bash", "-c",
        "printf '\\000new binary\\377' > binary.dat; printf '\\000created\\376' > created.dat; rm deleted.dat; chmod +x script.sh",
    ]);
    assert!(status.success(), "{stderr}");
    let (files, result) = support::read_capture(&capture);
    assert_eq!(result["capture"]["complete"], true);
    let changes = result["files"].as_array().unwrap();
    assert_eq!(changes.len(), 4);
    for (path, before, after) in [
        (
            "binary.dat",
            Some(b"\0old binary\xff\n".as_slice()),
            Some(b"\0new binary\xff".as_slice()),
        ),
        ("deleted.dat", Some(b"\0deleted\xfe".as_slice()), None),
        ("created.dat", None, Some(b"\0created\xfe".as_slice())),
    ] {
        let change = changes
            .iter()
            .find(|change| change["path"] == path)
            .unwrap();
        assert_eq!(
            change["before_sha256"],
            before
                .map(|bytes| serde_json::json!(format!("{:x}", Sha256::digest(bytes))))
                .unwrap_or(serde_json::Value::Null)
        );
        assert_eq!(
            change["after_sha256"],
            after
                .map(|bytes| serde_json::json!(format!("{:x}", Sha256::digest(bytes))))
                .unwrap_or(serde_json::Value::Null)
        );
        assert!(
            change["after_blob"].is_null(),
            "binary contents must be supplied by the patch"
        );
    }
    let patch = std::str::from_utf8(&files["changes.patch"]).unwrap();
    assert_eq!(patch.matches("GIT binary patch").count(), 3);
    assert!(patch.contains("old mode 100644\nnew mode 100755\n"));
    assert_eq!(
        fs::metadata(root.join("script.sh"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
    assert_eq!(
        fs::read(root.join("binary.dat")).unwrap(),
        b"\0new binary\xff"
    );
    repo.reverse_patch(&files["changes.patch"]);
    assert_eq!(
        fs::read(root.join("binary.dat")).unwrap(),
        b"\0old binary\xff\n"
    );
    assert_eq!(
        fs::read(root.join("deleted.dat")).unwrap(),
        b"\0deleted\xfe"
    );
    assert!(!root.join("created.dat").exists());
    assert_eq!(
        fs::metadata(root.join("script.sh"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o644
    );
    assert_eq!(repo.status(), "");
}
#[test]
fn streams_capture_on_fd_three_and_holds_the_lock_until_output_finishes() {
    let repo = Repository::new("duck pond");
    repo.write("duck.txt", "quack\n");
    repo.commit("Add duck");
    let after = "honk\n".repeat(200_000);
    let (mut first, mut reader) = repo.spawn_pipe_capture("stream", &[
        "run", "--", "bash", "-c",
        "if [ -e /dev/fd/3 ]; then printf 'FD 3 leaked\\n' >&2; exit 9; fi; awk 'BEGIN {for (i=0;i<200000;i++) print \"honk\"}' > duck.txt; printf 'capture ready\\n'; printf 'command stderr\\n' >&2",
    ]);
    first.wait_for_output("capture ready\n");
    let mut capture = support::read_pipe_chunk(&mut reader, 512);
    assert_eq!(capture.len(), 512, "Kao must begin emitting the archive");
    let mut second = repo.spawn_capture(
        "waiting",
        &[
            "run",
            "--",
            "bash",
            "-c",
            "printf 'second started\\n'; printf 'second\\n' >> duck.txt",
        ],
    );
    second.assert_waiting("second started\n");
    assert_eq!(repo.read("duck.txt"), after);
    loop {
        let bytes = support::read_pipe_chunk(&mut reader, 65536);
        if bytes.is_empty() {
            break;
        }
        capture.extend(bytes);
    }
    let (status, stdout, stderr, capture) = first.finish_stream(capture);
    assert!(status.success());
    assert_eq!(stdout, "capture ready\n");
    assert_eq!(stderr, "command stderr\n");
    let (files, result) = support::read_capture(&capture);
    assert_eq!(result["capture"]["complete"], true);
    let change = &result["files"][0];
    assert_eq!(
        change["after_sha256"],
        format!("{:x}", Sha256::digest(after.as_bytes()))
    );
    assert_eq!(
        files[change["after_blob"].as_str().unwrap()],
        after.as_bytes()
    );
    let (status, _, stderr, capture) = second.finish();
    assert!(status.success(), "{stderr}");
    let (_, second_result) = support::read_capture(&capture);
    assert_eq!(
        second_result["files"][0]["before_sha256"],
        change["after_sha256"]
    );
    assert_eq!(repo.read("duck.txt"), format!("{after}second\n"));
}
#[test]
fn reports_capture_failure_and_retains_available_artifacts() {
    use std::fs;

    let repo = Repository::new("duck pond");
    let root = repo.canonical_path();
    repo.write("a-good.txt", "before\n");
    repo.commit("Add files");
    let (status, _, stderr, capture) = repo.capture(&[
        "run",
        "--",
        "bash",
        "-c",
        "printf 'after\\n' > a-good.txt; git init -q nested; git -C nested -c user.name=a -c user.email=a@b -c commit.gpgsign=false commit -q --allow-empty -m nested",
    ]);
    assert_eq!(status.code(), Some(125));
    let (_, result) = support::read_capture(&capture);
    assert_eq!(
        result["command"]["exit_code"], 0,
        "the command succeeded even though capture failed"
    );
    assert_eq!(result["capture"]["complete"], false);
    assert!(
        result["capture"]["error"]
            .as_str()
            .unwrap()
            .contains("embedded repositories are not supported: nested")
    );
    let retained = support::retained_artifacts(&stderr);
    assert_eq!(fs::read(retained.join("capture.tar")).unwrap(), capture);
    assert_eq!(repo.read("a-good.txt"), "after\n");
    assert!(root.join("nested/.git").is_dir());
    let (status, _, stderr, capture) = repo.capture(&["run", "--", "true"]);
    assert!(status.success(), "failure must release the lock: {stderr}");
    let (_, result) = support::read_capture(&capture);
    assert_eq!(result["capture"]["complete"], true);
}
#[test]
fn retains_a_complete_capture_when_the_fd_three_reader_disconnects() {
    use std::fs;

    let repo = Repository::new("duck pond");
    repo.write("duck.txt", "quack\n");
    repo.commit("Add duck");
    let (running, reader) = repo.spawn_pipe_capture(
        "disconnected",
        &[
            "run",
            "--",
            "bash",
            "-c",
            "printf 'honk\\n' > duck.txt; exit 7",
        ],
    );
    drop(reader);
    let (status, stdout, stderr, _) = running.finish_stream(Vec::new());
    assert_eq!(status.code(), Some(125));
    assert_eq!(stdout, "");
    assert!(stderr.contains("capture output failed"), "{stderr}");
    let capture = fs::read(support::retained_artifacts(&stderr).join("capture.tar")).unwrap();
    let (files, result) = support::read_capture(&capture);
    assert_eq!(result["command"]["exit_code"], 7);
    assert_eq!(
        result["capture"]["complete"], true,
        "the retained archive must be complete even though delivery failed"
    );
    assert_eq!(
        files[result["files"][0]["after_blob"].as_str().unwrap()],
        b"honk\n"
    );
    assert_eq!(repo.read("duck.txt"), "honk\n");
    let (status, _, stderr, capture) =
        repo.capture(&["run", "--", "bash", "-c", "printf 'next\\n' > duck.txt"]);
    assert!(
        status.success(),
        "output failure must release the lock: {stderr}"
    );
    let (_, result) = support::read_capture(&capture);
    assert_eq!(
        result["files"][0]["before_sha256"],
        format!("{:x}", Sha256::digest(b"honk\n"))
    );
}
#[test]
fn rejects_unwritable_fd_three_before_executing_the_command() {
    let repo = Repository::new("duck pond");
    repo.write("duck.txt", "quack\n");
    repo.commit("Add duck");
    for readonly in [false, true] {
        let (status, stdout, stderr) = repo.capture_with_unwritable_descriptor(
            readonly,
            &[
                "run",
                "--",
                "bash",
                "-c",
                "printf 'executed\\n'; printf 'honk\\n' > duck.txt",
            ],
        );
        assert_eq!(status.code(), Some(125));
        assert_eq!(stdout, "");
        assert!(
            stderr.contains("FD 3 must be open for capture output"),
            "{stderr}"
        );
        assert_eq!(repo.read("duck.txt"), "quack\n");
        assert_eq!(repo.status(), "");
    }
}
#[test]
fn reports_empty_net_changes_without_resetting_dirty_files() {
    let repo = Repository::new("duck pond");
    repo.write("duck.txt", "committed\n");
    repo.write(".gitignore", "cache.txt\n");
    repo.commit("Add files");
    repo.write("duck.txt", "dirty\n");
    repo.write("untracked.txt", "private\n");
    repo.write("cache.txt", "cache\n");
    let before_status = repo.status();
    let mut previous_operation = String::new();
    for script in [
        "true",
        "printf 'temporary\\n' > duck.txt; printf 'dirty\\n' > duck.txt; touch untracked.txt; mkdir transient; printf 'temporary\\n' > transient/file; rm -r transient; printf 'updated cache\\n' > cache.txt",
    ] {
        let (status, stdout, stderr, capture) = repo.capture(&["run", "--", "bash", "-c", script]);
        assert!(status.success(), "{stderr}");
        assert_eq!(stdout, "");
        assert_eq!(stderr, "");
        let (files, result) = support::read_capture(&capture);
        assert_eq!(result["capture"]["complete"], true);
        assert_eq!(result["command"]["exit_code"], 0);
        assert!(result["files"].as_array().unwrap().is_empty());
        assert!(files["changes.patch"].is_empty());
        assert_eq!(
            files.len(),
            2,
            "no-change captures need only patch and manifest"
        );
        let operation = result["operation_id"].as_str().unwrap();
        assert!(!operation.is_empty());
        assert_ne!(operation, previous_operation);
        previous_operation = operation.to_owned();
        assert_eq!(repo.read("duck.txt"), "dirty\n");
        assert_eq!(repo.read("untracked.txt"), "private\n");
        assert_eq!(repo.status(), before_status);
    }
    assert_eq!(repo.read("cache.txt"), "updated cache\n");
}
#[test]
fn cleans_up_snapshots_without_following_ignored_symlinks() {
    use std::fs;
    use std::os::unix::fs::symlink;

    let repo = Repository::new("duck pond");
    let root = repo.canonical_path();
    let parent = root.parent().unwrap();
    let protected = parent.join("protected");
    fs::create_dir(&protected).unwrap();
    fs::write(protected.join("outside.txt"), b"keep\n").unwrap();
    repo.write("duck.txt", "quack\n");
    repo.write(".gitignore", "dependencies/\n");
    repo.commit("Add files");
    fs::create_dir(root.join("dependencies")).unwrap();
    symlink(&protected, root.join("dependencies/outside")).unwrap();
    let directories = || {
        let mut paths: Vec<_> = fs::read_dir(parent)
            .unwrap()
            .map(Result::unwrap)
            .filter(|entry| entry.file_type().unwrap().is_dir())
            .map(|entry| entry.path())
            .collect();
        paths.sort();
        paths
    };
    let before = directories();
    let (status, _, stderr, capture) = repo.capture(&["run", "--", "true"]);
    assert!(status.success(), "{stderr}");
    let (_, result) = support::read_capture(&capture);
    assert_eq!(result["capture"]["complete"], true);
    assert_eq!(
        directories(),
        before,
        "captures must not write next to the repository"
    );
    assert_eq!(
        fs::read_dir(root.join(".git/kao/operations"))
            .unwrap()
            .count(),
        0,
        "successful captures must remove their operation artifacts"
    );
    assert_eq!(fs::read(protected.join("outside.txt")).unwrap(), b"keep\n");
    assert!(
        fs::symlink_metadata(root.join("dependencies/outside"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(repo.status(), "");
}
#[test]
fn preserves_nested_cwd_and_discovers_linked_working_trees() {
    use std::fs;

    let repo = Repository::new("duck pond\nwith newline");
    let root = repo.canonical_path();
    fs::create_dir(root.join("nested")).unwrap();
    repo.write("nested/duck.txt", "quack\n");
    repo.commit("Add nested file");
    let linked = repo.add_worktree("linked pond");
    for working_tree in [&root, &linked] {
        let cwd = working_tree.join("nested").canonicalize().unwrap();
        fs::write(cwd.join("duck.txt"), b"dirty before\n").unwrap();
        let (status, stdout, stderr, capture) = repo.capture_from(
            &cwd,
            &[
                "run",
                "--",
                "bash",
                "-c",
                "pwd -P; printf 'honk\\n' > duck.txt",
            ],
        );
        assert!(status.success(), "{stderr}");
        assert_eq!(stdout, format!("{}\n", cwd.display()));
        let (files, result) = support::read_capture(&capture);
        assert_eq!(
            result["repository_root"].as_str().unwrap(),
            working_tree.to_str().unwrap()
        );
        assert_eq!(result["cwd"].as_str().unwrap(), cwd.to_str().unwrap());
        assert_eq!(result["capture"]["complete"], true);
        assert_eq!(result["files"].as_array().unwrap().len(), 1);
        let change = &result["files"][0];
        assert_eq!(change["path"], "nested/duck.txt");
        assert_eq!(
            change["before_sha256"],
            format!("{:x}", Sha256::digest(b"dirty before\n"))
        );
        assert_eq!(files[change["after_blob"].as_str().unwrap()], b"honk\n");
        assert_eq!(fs::read(cwd.join("duck.txt")).unwrap(), b"honk\n");
    }
}
#[test]
fn captures_raw_bytes_despite_line_ending_conversion_and_filters() {
    let repo = Repository::new("duck pond");
    repo.config("core.autocrlf", "true");
    repo.config("filter.upper.clean", "tr a-z A-Z");
    repo.write(".gitattributes", "*.txt text eol=lf\n*.up filter=upper\n");
    repo.write("duck.txt", "quack\r\nquack\r\n");
    repo.write("duck.up", "lower\n");
    repo.commit("Add converted files");

    let (status, _, stderr, capture) = repo.capture(&[
        "run",
        "--",
        "bash",
        "-c",
        "printf 'honk\\r\\nquack\\r\\n' > duck.txt; printf 'still lower\\n' > duck.up",
    ]);
    assert!(status.success(), "{stderr}");
    let (files, result) = support::read_capture(&capture);
    assert_eq!(result["capture"]["complete"], true);
    let changes = result["files"].as_array().unwrap();
    assert_eq!(changes.len(), 2);
    for (path, before, after) in [
        ("duck.txt", "quack\r\nquack\r\n", "honk\r\nquack\r\n"),
        ("duck.up", "lower\n", "still lower\n"),
    ] {
        let change = changes
            .iter()
            .find(|change| change["path"] == path)
            .unwrap();
        assert_eq!(
            change["before_sha256"],
            format!("{:x}", Sha256::digest(before.as_bytes()))
        );
        assert_eq!(
            change["after_sha256"],
            format!("{:x}", Sha256::digest(after.as_bytes()))
        );
        let blob = &files[change["after_blob"].as_str().unwrap()];
        assert_eq!(blob, after.as_bytes());
        assert_eq!(
            repo.reconstruct_before(&files["changes.patch"], path, blob),
            before.as_bytes()
        );
    }
}
#[test]
fn each_capture_starts_from_the_current_files() {
    let repo = Repository::new("duck pond");
    repo.write("duck.txt", "quack\n");
    repo.commit("Add duck");
    let mut expected_before = "quack\n".to_owned();
    for (index, outside_edit) in [None, Some("edited outside Kao\n"), None]
        .into_iter()
        .enumerate()
    {
        if let Some(contents) = outside_edit {
            repo.write("duck.txt", contents);
            expected_before = contents.to_owned();
        }
        let after = format!("honk {index}\n");
        let (status, _, stderr, capture) = repo.capture(&[
            "run",
            "--",
            "bash",
            "-c",
            "printf '%s' \"$1\" > duck.txt",
            "edit",
            &after,
        ]);
        assert!(status.success(), "{stderr}");
        let (_, result) = support::read_capture(&capture);
        assert_eq!(result["files"].as_array().unwrap().len(), 1);
        assert_eq!(
            result["files"][0]["before_sha256"],
            format!("{:x}", Sha256::digest(expected_before.as_bytes())),
            "capture {index} must start from the bytes on disk"
        );
        assert_eq!(
            result["files"][0]["after_sha256"],
            format!("{:x}", Sha256::digest(after.as_bytes()))
        );
        expected_before = after;
    }
}
#[test]
fn captures_symlink_changes_without_touching_their_targets() {
    use std::fs;
    use std::os::unix::fs::symlink;

    let repo = Repository::new("duck pond");
    let root = repo.canonical_path();
    let outside = root.parent().unwrap().join("outside.txt");
    fs::write(&outside, b"outside\n").unwrap();
    repo.write("duck.txt", "quack\n");
    symlink("duck.txt", root.join("retargeted")).unwrap();
    symlink("duck.txt", root.join("removed")).unwrap();
    repo.commit("Add links");

    let (status, _, stderr, capture) = repo.capture(&[
        "run",
        "--",
        "bash",
        "-c",
        "ln -sfn ../outside.txt retargeted; rm removed; ln -s duck.txt created",
    ]);
    assert!(status.success(), "{stderr}");
    let (files, result) = support::read_capture(&capture);
    assert_eq!(result["capture"]["complete"], true);
    let changes = result["files"].as_array().unwrap();
    assert_eq!(changes.len(), 3);
    for (path, before, after) in [
        ("retargeted", Some("duck.txt"), Some("../outside.txt")),
        ("removed", Some("duck.txt"), None),
        ("created", None, Some("duck.txt")),
    ] {
        let change = changes
            .iter()
            .find(|change| change["path"] == path)
            .unwrap();
        let digest = |target: Option<&str>| {
            target
                .map(|target| serde_json::json!(format!("{:x}", Sha256::digest(target.as_bytes()))))
                .unwrap_or(serde_json::Value::Null)
        };
        assert_eq!(change["before_sha256"], digest(before), "{path}");
        assert_eq!(change["after_sha256"], digest(after), "{path}");
        assert!(
            change["after_blob"].is_null(),
            "symlinks never have after blobs"
        );
    }
    assert_eq!(fs::read(&outside).unwrap(), b"outside\n");
    let patch = std::str::from_utf8(&files["changes.patch"]).unwrap();
    assert!(patch.contains("diff --git a/removed b/removed\ndeleted file mode 120000\n"));
    assert!(patch.contains("diff --git a/created b/created\nnew file mode 120000\n"));
    repo.reverse_patch_excluding(&files["changes.patch"], &["removed"]);
    assert_eq!(
        fs::read_link(root.join("retargeted")).unwrap(),
        std::path::Path::new("duck.txt")
    );
    assert!(fs::symlink_metadata(root.join("created")).is_err());
    assert_eq!(repo.status(), " D removed\n");
}

#[test]
fn kao_lock_waits_for_a_running_capture() {
    let repo = Repository::new("duck pond");
    repo.write("duck.txt", "quack\n");
    repo.commit("Add duck");
    let mut capture = repo.spawn_capture(
        "capture",
        &[
            "run",
            "--",
            "bash",
            "-c",
            "printf 'capture started\\n'; read -r release",
        ],
    );
    capture.wait_for_output("capture started\n");
    let mut locked = repo.spawn_capture(
        "locked",
        &[
            "lock",
            "--",
            "bash",
            "-c",
            "printf 'lock started\\n'; printf 'locked\\n' > duck.txt",
        ],
    );
    locked.assert_waiting("lock started\n");
    capture.release();
    let (status, _, stderr, capture) = capture.finish();
    assert!(status.success(), "{stderr}");
    let (_, result) = support::read_capture(&capture);
    assert!(
        result["files"].as_array().unwrap().is_empty(),
        "the later lock edit must not enter this capture"
    );
    let (status, stdout, stderr, _) = locked.finish();
    assert!(status.success(), "{stderr}");
    assert_eq!(stdout, "invoking Kao\nlock started\n");
    assert_eq!(repo.read("duck.txt"), "locked\n");
}

#[test]
fn kao_run_waits_for_kao_lock_and_excludes_its_changes() {
    let repo = Repository::new("duck pond");
    repo.write("duck.txt", "quack\n");
    repo.commit("Add duck");
    let mut locked = repo.spawn_capture(
        "locked",
        &[
            "lock",
            "--",
            "bash",
            "-c",
            "printf 'locked\\n' > duck.txt; printf 'lock started\\n'; read -r release",
        ],
    );
    locked.wait_for_output("lock started\n");
    let mut capture = repo.spawn_capture(
        "capture",
        &[
            "run",
            "--",
            "bash",
            "-c",
            "printf 'capture started\\n'; printf 'run\\n' >> duck.txt",
        ],
    );
    capture.assert_waiting("capture started\n");
    locked.release();
    let (status, _, stderr, _) = locked.finish();
    assert!(status.success(), "{stderr}");
    let (status, _, stderr, capture) = capture.finish();
    assert!(status.success(), "{stderr}");
    let (_, result) = support::read_capture(&capture);
    assert_eq!(result["files"].as_array().unwrap().len(), 1);
    assert_eq!(
        result["files"][0]["before_sha256"],
        format!("{:x}", Sha256::digest(b"locked\n")),
        "the capture must start after the change made under kao lock"
    );
    assert_eq!(
        result["files"][0]["after_sha256"],
        format!("{:x}", Sha256::digest(b"locked\nrun\n"))
    );
}

#[test]
fn kao_lock_passes_through_input_output_and_exit_code() {
    let repo = Repository::new("duck pond");
    repo.write("duck.txt", "quack\n");
    repo.commit("Add duck");
    let (status, stdout, stderr) = repo.kao_in(
        &repo.canonical_path(),
        &[
            "lock",
            "--",
            "bash",
            "-c",
            "read -r line; printf 'out %s\\n' \"$line\"; printf 'err\\n' >&2; exit 7",
        ],
        b"duck\n",
    );
    assert_eq!(status.code(), Some(7));
    assert_eq!(stdout, "out duck\n");
    assert_eq!(stderr, "err\n");
}

#[test]
fn kao_lock_requires_a_git_working_tree() {
    let repo = Repository::new("duck pond");
    let outside = repo.outside();
    let (status, stdout, stderr) = repo.kao_in(
        &outside,
        &["lock", "--", "bash", "-c", "printf 'ran\\n'; touch ran"],
        b"",
    );
    assert_eq!(status.code(), Some(125));
    assert_eq!(stdout, "");
    assert!(stderr.starts_with("kao: "), "{stderr}");
    assert!(!stderr.contains("usage"), "{stderr}");
    assert!(!outside.join("ran").exists());
}

#[test]
fn captures_changes_in_sha256_repositories() {
    let repo = Repository::with_object_format("duck pond", "sha256");
    repo.write("duck.txt", "quack\r\n");
    repo.commit("Add duck");
    let (status, _, stderr, capture) =
        repo.capture(&["run", "--", "bash", "-c", "printf 'honk\\r\\n' > duck.txt"]);
    assert!(status.success(), "{stderr}");
    let (files, result) = support::read_capture(&capture);
    assert_eq!(result["capture"]["complete"], true);
    assert_eq!(result["files"][0]["path"], "duck.txt");
    assert_eq!(
        files[result["files"][0]["after_blob"].as_str().unwrap()],
        b"honk\r\n"
    );
    repo.reverse_patch(&files["changes.patch"]);
    assert_eq!(repo.read("duck.txt"), "quack\r\n");
}

#[test]
fn rejects_malformed_invocations_with_usage() {
    let repo = Repository::new("duck pond");
    repo.write("duck.txt", "quack\n");
    repo.commit("Add duck");
    for args in [
        &[][..],
        &["frobnicate"][..],
        &["run"][..],
        &["run", "--"][..],
        &["run", "touch", "duck.txt"][..],
        &["lock"][..],
        &["lock", "--"][..],
        &["lock", "touch", "duck.txt"][..],
    ] {
        let (status, stdout, stderr) = repo.kao_outcome(args);
        assert_eq!(status.code(), Some(125), "{args:?}");
        assert_eq!(stdout, "", "{args:?}");
        assert!(stderr.contains("usage: kao run -- "), "{args:?}: {stderr}");
    }
    assert_eq!(repo.status(), "");
}
#[test]
fn requires_a_git_version_with_attribute_source_support() {
    let repo = Repository::new("duck pond");
    repo.write("duck.txt", "quack\n");
    repo.commit("Add duck");
    let fake = repo.fake_git("2.40.1");
    let (status, stdout, stderr, _) = repo.capture_with_path(
        &repo.canonical_path(),
        Some(&fake),
        &["run", "--", "bash", "-c", "printf 'honk\\n' > duck.txt"],
    );
    assert_eq!(status.code(), Some(125));
    assert_eq!(stdout, "");
    assert!(
        stderr.contains("Kao requires Git 2.41 or newer; found 2.40.1"),
        "{stderr}"
    );
    assert_eq!(repo.read("duck.txt"), "quack\n");
}
