mod support;

#[cfg(target_os = "macos")]
use sha2::{Digest, Sha256};
#[cfg(target_os = "macos")]
use std::collections::BTreeMap;
#[cfg(target_os = "macos")]
use std::io::Read;
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

    let mut archive = tar::Archive::new(capture.as_slice());
    let mut files = BTreeMap::new();
    let mut last_path = String::new();
    for entry in archive.entries().expect("capture must be a tar archive") {
        let mut entry = entry.expect("capture entry must be readable");
        if entry.header().entry_type().is_dir() {
            continue;
        }
        let path = entry.path().unwrap().to_str().unwrap().to_owned();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).unwrap();
        assert!(
            files.insert(path.clone(), bytes).is_none(),
            "duplicate archive entry: {path}"
        );
        last_path = path;
    }
    assert_eq!(
        last_path, "result.json",
        "manifest must finalize the capture"
    );
    let result: serde_json::Value = serde_json::from_slice(
        files
            .get("result.json")
            .expect("capture must contain result.json"),
    )
    .unwrap();
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
fn kao_says_hello_to_a_duck() {
    let repo = Repository::new("duck pond");
    repo.write("duck.txt", "quack\n");
    repo.commit("Add duck");

    assert_eq!(repo.kao(&[]), "Hello, world!\n");
    assert_eq!(repo.read("duck.txt"), "quack\n");
    assert_eq!(repo.status(), "");
}
