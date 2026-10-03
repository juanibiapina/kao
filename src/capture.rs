use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Output};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::native::clone_path;
use crate::watcher::Watcher;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Default)]
struct CapturedFiles {
    patch: Vec<u8>,
    files: Vec<Value>,
    blobs: BTreeMap<String, Vec<u8>>,
}

fn git(root: &Path) -> Command {
    let mut command = Command::new("git");
    command.current_dir(root).env("GIT_OPTIONAL_LOCKS", "0");
    command
}

fn checked(command: &mut Command) -> Result<Output> {
    let output = command.output()?;
    if !output.status.success() {
        return Err(format!(
            "Git failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    Ok(output)
}

fn discover(cwd: &Path) -> Result<(PathBuf, PathBuf)> {
    let (repository, _) = gix_discover::upwards(cwd).map_err(|error| error.to_string())?;
    let (git_dir, worktree) = repository.into_repository_and_work_tree_directories();
    let root = worktree
        .ok_or("Kao requires a Git working tree")?
        .canonicalize()?;
    let common =
        match gix_discover::path::from_plain_file_relative_to_file(&git_dir.join("commondir")) {
            Some(common) => common?,
            None => git_dir,
        };
    Ok((root, common.canonicalize()?))
}

fn result_output() -> Result<File> {
    let flags = unsafe { libc::fcntl(3, libc::F_GETFL) };
    if flags == -1 || flags & libc::O_ACCMODE == libc::O_RDONLY {
        return Err("FD 3 must be open for capture output".into());
    }
    let descriptor_flags = unsafe { libc::fcntl(3, libc::F_GETFD) };
    if descriptor_flags == -1
        || unsafe { libc::fcntl(3, libc::F_SETFD, descriptor_flags | libc::FD_CLOEXEC) } == -1
    {
        return Err(io::Error::last_os_error().into());
    }
    Ok(unsafe { File::from_raw_fd(3) })
}

fn lock_repository(git_dir: &Path) -> Result<File> {
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(git_dir.join("kao.lock"))?;
    loop {
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } == 0 {
            return Ok(lock);
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error.into());
        }
    }
}

fn scoped_paths(root: &Path, candidates: Option<&BTreeSet<PathBuf>>) -> Result<BTreeSet<PathBuf>> {
    if candidates.is_some_and(BTreeSet::is_empty) {
        return Ok(BTreeSet::new());
    }
    let mut command = git(root);
    command.args([
        "ls-files",
        "-z",
        "--cached",
        "--others",
        "--exclude-standard",
        "--",
    ]);
    for path in candidates.into_iter().flatten() {
        let mut literal = OsString::from(":(literal)");
        literal.push(path);
        command.arg(literal);
    }
    let output = checked(&mut command)?;
    Ok(output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| PathBuf::from(OsStr::from_bytes(path)))
        .collect())
}

struct Snapshot {
    directory: PathBuf,
    scope: BTreeSet<PathBuf>,
}

impl Snapshot {
    fn create(root: &Path, directory: PathBuf) -> Result<Self> {
        let scope = scoped_paths(root, None)?;
        fs::create_dir(&directory)?;
        let entries: BTreeSet<_> = scope
            .iter()
            .filter_map(|path| path.components().next())
            .map(|entry| PathBuf::from(entry.as_os_str()))
            .collect();
        for entry in entries {
            let source = root.join(&entry);
            match fs::symlink_metadata(&source) {
                Ok(_) => clone_path(&source, &directory.join(&entry))
                    .map_err(|error| format!("mandatory copy-on-write clone failed: {error}"))?,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(Self { directory, scope })
    }

    fn candidates(&self, changed: &BTreeSet<PathBuf>) -> BTreeSet<PathBuf> {
        let mut paths = BTreeSet::new();
        for changed in changed {
            for path in self.scope.range(changed.clone()..) {
                if !path.starts_with(changed) {
                    break;
                }
                paths.insert(path.clone());
            }
        }
        paths
    }
}

fn freeze(source: &Path, destination: &Path) -> Result<Option<(Vec<u8>, u32)>> {
    let metadata = match fs::symlink_metadata(source) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_file() {
        return Err(format!("unsupported file type: {}", source.display()).into());
    }
    fs::create_dir_all(destination.parent().ok_or("file has no parent")?)?;
    clone_path(source, destination)?;
    let metadata = fs::metadata(destination)?;
    Ok(Some((
        fs::read(destination)?,
        metadata.permissions().mode() & 0o111,
    )))
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn quote_path(path: &str) -> String {
    if !path
        .bytes()
        .any(|byte| !(32..127).contains(&byte) || byte == b'"' || byte == b'\\')
    {
        return path.to_owned();
    }
    let mut quoted = String::from("\"");
    for byte in path.bytes() {
        match byte {
            b'"' => quoted.push_str("\\\""),
            b'\\' => quoted.push_str("\\\\"),
            b'\n' => quoted.push_str("\\n"),
            b'\r' => quoted.push_str("\\r"),
            b'\t' => quoted.push_str("\\t"),
            7 => quoted.push_str("\\a"),
            8 => quoted.push_str("\\b"),
            11 => quoted.push_str("\\v"),
            12 => quoted.push_str("\\f"),
            32..=126 => quoted.push(char::from(byte)),
            _ => quoted.push_str(&format!("\\{byte:03o}")),
        }
    }
    quoted.push('"');
    quoted
}

fn patch(root: &Path, paths: &[(String, bool, bool)]) -> Result<Vec<u8>> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let output = git(root)
        .args([
            "-c",
            "core.quotePath=true",
            "diff",
            "--no-index",
            "--binary",
            "--no-renames",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            "--",
            "before",
            "after",
        ])
        .output()?;
    if !matches!(output.status.code(), Some(0 | 1)) {
        return Err(format!(
            "Git diff failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let mut replacements = BTreeMap::new();
    for (path, before, after) in paths {
        let left_dir = if *before { "before" } else { "after" };
        let right_dir = if *after { "after" } else { "before" };
        let left = quote_path(&format!("a/{left_dir}/{path}"));
        let right = quote_path(&format!("b/{right_dir}/{path}"));
        let target_left = quote_path(&format!("a/{path}"));
        let target_right = quote_path(&format!("b/{path}"));
        replacements.insert(
            format!("diff --git {left} {right}\n"),
            format!("diff --git {target_left} {target_right}\n"),
        );
        for (prefix, source, target) in
            [("---", &left, &target_left), ("+++", &right, &target_right)]
        {
            for suffix in ["\n", "\t\n"] {
                replacements.insert(
                    format!("{prefix} {source}{suffix}"),
                    format!("{prefix} {target}{suffix}"),
                );
            }
        }
    }
    let mut result = Vec::new();
    let mut headers = true;
    for line in output.stdout.split_inclusive(|byte| *byte == b'\n') {
        if line.starts_with(b"diff --git ") {
            headers = true;
        } else if line.starts_with(b"@@") || line.starts_with(b"GIT binary patch") {
            headers = false;
        }
        if headers
            && let Ok(line_text) = std::str::from_utf8(line)
            && let Some(replacement) = replacements.get(line_text)
        {
            result.extend_from_slice(replacement.as_bytes());
        } else {
            result.extend_from_slice(line);
        }
    }
    Ok(result)
}

fn append(archive: &mut tar::Builder<File>, name: &str, contents: &[u8]) -> Result<()> {
    let mut header = tar::Header::new_gnu();
    header.set_size(contents.len() as u64);
    header.set_mode(0o600);
    header.set_cksum();
    archive.append_data(&mut header, name, contents)?;
    Ok(())
}

fn capture_files(
    root: &Path,
    snapshot: &Snapshot,
    artifacts: &Path,
    watcher: Watcher,
) -> Result<CapturedFiles> {
    let candidates: BTreeSet<_> = watcher
        .finish()?
        .into_iter()
        .filter_map(|path| path.strip_prefix(root).ok().map(Path::to_path_buf))
        .filter(|path| !path.as_os_str().is_empty() && !path.starts_with(".git"))
        .collect();
    let mut paths = snapshot.candidates(&candidates);
    let unknown: BTreeSet<_> = candidates
        .into_iter()
        .filter(|path| !snapshot.scope.contains(path))
        .collect();
    paths.extend(scoped_paths(root, Some(&unknown))?);
    let before_dir = artifacts.join("before");
    let after_dir = artifacts.join("after");
    fs::create_dir(&before_dir)?;
    fs::create_dir(&after_dir)?;
    let mut files = Vec::new();
    let mut blobs = BTreeMap::new();
    let mut patch_paths = Vec::new();
    for path in paths {
        let name = path
            .to_str()
            .ok_or("capture paths must be UTF-8")?
            .to_owned();
        let before_path = snapshot.directory.join(&path);
        let after_path = root.join(&path);
        if fs::symlink_metadata(&before_path)
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
            && fs::symlink_metadata(&after_path)
                .is_ok_and(|metadata| metadata.file_type().is_symlink())
            && fs::read_link(&before_path)? == fs::read_link(&after_path)?
        {
            continue;
        }
        let before = freeze(&before_path, &before_dir.join(&path))?;
        let after = freeze(&after_path, &after_dir.join(&path))?;
        if before == after {
            if before.is_some() {
                fs::remove_file(before_dir.join(&path))?;
            }
            if after.is_some() {
                fs::remove_file(after_dir.join(&path))?;
            }
            continue;
        }
        let before_hash = before.as_ref().map(|(bytes, _)| hash(bytes));
        let after_hash = after.as_ref().map(|(bytes, _)| hash(bytes));
        let blob = if before.is_some()
            && let Some((bytes, _)) = &after
            && !bytes.iter().take(8000).any(|byte| *byte == 0)
        {
            let name = format!("blobs/{}", after_hash.as_ref().ok_or("missing after hash")?);
            blobs.insert(name.clone(), bytes.clone());
            Some(name)
        } else {
            None
        };
        files.push(json!({"path": name, "before_sha256": before_hash, "after_sha256": after_hash, "after_blob": blob}));
        patch_paths.push((name, before.is_some(), after.is_some()));
    }
    let changes = patch(artifacts, &patch_paths)?;
    Ok(CapturedFiles {
        patch: changes,
        files,
        blobs,
    })
}

pub fn run(args: &[OsString]) -> Result<i32> {
    if args.len() < 3 || args[0] != "run" || args[1] != "--" {
        return Err("usage: kao run -- command [args...] 3>capture.tar".into());
    }
    let mut output = result_output()?;
    let cwd = std::env::current_dir()?.canonicalize()?;
    let (root, git_dir) = discover(&cwd)?;
    let _lock = lock_repository(&git_dir)?;
    let artifacts = tempfile::Builder::new()
        .prefix(".kao-")
        .tempdir_in(root.parent().ok_or("repository has no parent")?)?;
    let operation_id = artifacts
        .path()
        .file_name()
        .ok_or("missing operation ID")?
        .to_string_lossy()
        .into_owned();
    let watcher = Watcher::start(&root, &git_dir)?;
    let snapshot = Snapshot::create(&root, artifacts.path().join("snapshot"))?;
    let command = Command::new(&args[2])
        .args(&args[3..])
        .current_dir(&cwd)
        .status();
    let command_result = match &command {
        Ok(status) => json!({"exit_code": status.code(), "signal": status.signal()}),
        Err(error) => json!({"exit_code": null, "signal": null, "error": error.to_string()}),
    };
    let captured = capture_files(&root, &snapshot, artifacts.path(), watcher);
    let (captured, error) = match captured {
        Ok(captured) => (captured, None),
        Err(error) => (CapturedFiles::default(), Some(error.to_string())),
    };
    let complete = error.is_none();
    let manifest = json!({
        "format_version": 1, "operation_id": operation_id,
        "repository_root": root, "cwd": cwd, "command": command_result,
        "capture": {"complete": complete, "error": error}, "files": captured.files,
    });
    let finalized = (|| -> Result<()> {
        let archive_path = artifacts.path().join("capture.tar");
        let mut archive = tar::Builder::new(File::create(&archive_path)?);
        append(&mut archive, "changes.patch", &captured.patch)?;
        for (name, bytes) in captured.blobs {
            append(&mut archive, &name, &bytes)?;
        }
        append(
            &mut archive,
            "result.json",
            &serde_json::to_vec_pretty(&manifest)?,
        )?;
        archive.finish()?;
        drop(archive);
        io::copy(&mut File::open(&archive_path)?, &mut output)?;
        output.flush()?;
        Ok(())
    })();
    if let Err(error) = finalized {
        let retained = artifacts.keep();
        return Err(format!(
            "capture output failed: {error}; artifacts retained at {}",
            retained.display()
        )
        .into());
    }
    if !complete {
        let retained = artifacts.keep();
        return Err(format!(
            "capture failed: {}; artifacts retained at {}",
            error.as_deref().unwrap_or("unknown error"),
            retained.display()
        )
        .into());
    }
    let status: ExitStatus =
        command.map_err(|error| format!("command could not start: {error}"))?;
    Ok(status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(1)))
}
