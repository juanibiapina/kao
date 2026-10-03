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

#[test]
fn kao_says_hello_to_a_duck() {
    let repo = Repository::new("duck pond");
    repo.write("duck.txt", "quack\n");
    repo.commit("Add duck");

    assert_eq!(repo.kao(&[]), "Hello, world!\n");
    assert_eq!(repo.read("duck.txt"), "quack\n");
    assert_eq!(repo.status(), "");
}
