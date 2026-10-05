use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::{Command, ExitStatus};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::snapshot::{Change, Result, Workspace, check_git_version};

const REGULAR_FILE: u32 = 0o100000;

#[derive(Default)]
struct CapturedFiles {
    patch: Vec<u8>,
    files: Vec<Value>,
    blobs: BTreeMap<String, Vec<u8>>,
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

fn lock_repository(common_dir: &Path) -> Result<File> {
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(common_dir.join("kao.lock"))?;
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

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn is_regular(entry: Option<&crate::snapshot::Entry>) -> bool {
    entry.is_some_and(|entry| entry.mode & 0o170000 == REGULAR_FILE)
}

fn describe(workspace: &Workspace, changes: Vec<Change>, patch: Vec<u8>) -> Result<CapturedFiles> {
    let mut files = Vec::new();
    let mut blobs = BTreeMap::new();
    for change in changes {
        if let Some(after) = &change.after
            && workspace.disk_bytes(&change.path)? != after.bytes
        {
            return Err(format!(
                "captured contents differ from the file on disk: {}",
                change.path
            )
            .into());
        }
        let before_hash = change.before.as_ref().map(|entry| hash(&entry.bytes));
        let after_hash = change.after.as_ref().map(|entry| hash(&entry.bytes));
        let blob = if change.before.is_some()
            && is_regular(change.after.as_ref())
            && let Some(after) = &change.after
            && !after.bytes.iter().take(8000).any(|byte| *byte == 0)
        {
            let name = format!("blobs/{}", after_hash.as_ref().ok_or("missing after hash")?);
            blobs.insert(name.clone(), after.bytes.clone());
            Some(name)
        } else {
            None
        };
        files.push(json!({
            "path": change.path,
            "before_sha256": before_hash,
            "after_sha256": after_hash,
            "after_blob": blob,
        }));
    }
    Ok(CapturedFiles {
        patch,
        files,
        blobs,
    })
}

fn append(archive: &mut tar::Builder<File>, name: &str, contents: &[u8]) -> Result<()> {
    let mut header = tar::Header::new_gnu();
    header.set_size(contents.len() as u64);
    header.set_mode(0o600);
    header.set_cksum();
    archive.append_data(&mut header, name, contents)?;
    Ok(())
}

pub fn run(args: &[OsString]) -> Result<i32> {
    if args.len() < 3 || args[0] != "run" || args[1] != "--" {
        return Err("usage: kao run -- command [args...] 3>capture.tar".into());
    }
    check_git_version()?;
    let mut output = result_output()?;
    let cwd = std::env::current_dir()?.canonicalize()?;
    let workspace = Workspace::discover(&cwd)?;
    let _lock = lock_repository(&workspace.common_dir)?;
    workspace.bound_store()?;
    let operations = workspace.common_dir.join("kao").join("operations");
    fs::create_dir_all(&operations)?;
    let artifacts = tempfile::Builder::new()
        .prefix("op-")
        .tempdir_in(&operations)?;
    let operation_id = artifacts
        .path()
        .file_name()
        .ok_or("missing operation ID")?
        .to_string_lossy()
        .into_owned();
    let before = workspace.snapshot()?;
    fs::write(artifacts.path().join("before-tree"), &before.0)?;
    let command = Command::new(&args[2])
        .args(&args[3..])
        .current_dir(&cwd)
        .status();
    let command_result = match &command {
        Ok(status) => json!({"exit_code": status.code(), "signal": status.signal()}),
        Err(error) => json!({"exit_code": null, "signal": null, "error": error.to_string()}),
    };
    let captured = workspace.snapshot().and_then(|after| {
        let (changes, patch) = workspace.changes(&before, &after)?;
        describe(&workspace, changes, patch)
    });
    let (captured, error) = match captured {
        Ok(captured) => (captured, None),
        Err(error) => (CapturedFiles::default(), Some(error.to_string())),
    };
    let complete = error.is_none();
    let manifest = json!({
        "format_version": 1, "operation_id": operation_id,
        "repository_root": workspace.root, "cwd": cwd, "command": command_result,
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
    artifacts.close()?;
    let status: ExitStatus =
        command.map_err(|error| format!("command could not start: {error}"))?;
    Ok(status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(1)))
}
