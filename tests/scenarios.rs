mod support;

#[cfg(target_os = "macos")]
use sha2::{Digest, Sha256};
use support::Repository;

#[cfg(target_os = "macos")]
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

#[cfg(target_os = "macos")]
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

#[cfg(target_os = "macos")]
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

#[cfg(target_os = "macos")]
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

#[cfg(target_os = "macos")]
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

#[cfg(target_os = "macos")]
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

#[cfg(target_os = "macos")]
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

#[cfg(target_os = "macos")]
#[test]
fn reports_capture_failure_and_retains_available_artifacts() {
    use std::fs;

    let repo = Repository::new("duck pond");
    let root = repo.canonical_path();
    repo.write("a-good.txt", "before\n");
    repo.write("duck.txt", "quack\n");
    repo.commit("Add files");
    fs::write(root.parent().unwrap().join("outside.txt"), b"outside\n").unwrap();
    let (status, _, stderr, capture) = repo.capture(&[
        "run",
        "--",
        "bash",
        "-c",
        "printf 'after\\n' > a-good.txt; rm duck.txt; ln -s ../outside.txt duck.txt",
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
            .contains("unsupported file type")
    );
    let retained = std::path::PathBuf::from(
        stderr
            .split("artifacts retained at ")
            .nth(1)
            .expect("capture error must identify retained artifacts")
            .trim(),
    );
    assert_eq!(
        fs::read(retained.join("snapshot/duck.txt")).unwrap(),
        b"quack\n"
    );
    assert_eq!(
        fs::read(retained.join("before/a-good.txt")).unwrap(),
        b"before\n"
    );
    assert_eq!(
        fs::read(retained.join("after/a-good.txt")).unwrap(),
        b"after\n"
    );
    assert_eq!(fs::read(retained.join("capture.tar")).unwrap(), capture);
    assert_eq!(repo.read("a-good.txt"), "after\n");
    assert_eq!(
        fs::read(root.parent().unwrap().join("outside.txt")).unwrap(),
        b"outside\n"
    );
    assert!(
        fs::symlink_metadata(root.join("duck.txt"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    let (status, _, stderr, capture) = repo.capture(&["run", "--", "true"]);
    assert!(status.success(), "failure must release the lock: {stderr}");
    let (_, result) = support::read_capture(&capture);
    assert_eq!(result["capture"]["complete"], true);
}

#[test]
fn kao_says_hello_to_a_duck() {
    let repo = Repository::new("duck pond");
    repo.write("duck.txt", "quack\n");
    repo.commit("Add duck");

    assert_eq!(repo.kao(&[]), "Hello, world!\n");
    assert_eq!(repo.read("duck.txt"), "quack\n");
    assert_eq!(repo.status(), "");
}
