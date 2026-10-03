use std::fs::{self, File};
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(target_os = "macos")]
pub fn read_capture(
    capture: &[u8],
) -> (
    std::collections::BTreeMap<String, Vec<u8>>,
    serde_json::Value,
) {
    use std::io::Read;

    let mut archive = tar::Archive::new(capture);
    let mut files = std::collections::BTreeMap::new();
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
    let result = serde_json::from_slice(
        files
            .get("result.json")
            .expect("capture must contain result.json"),
    )
    .unwrap();
    (files, result)
}

fn run(root: &Path, command: &mut Command) -> (ExitStatus, String, String) {
    let result = execute(root, command);
    assert!(result.0.success(), "fixture: {}", root.display());
    result
}

fn configure(root: &Path, command: &mut Command) {
    let stdout = root.join("stdout.log");
    let stderr = root.join("stderr.log");
    command
        .env("HOME", root)
        .env("XDG_CONFIG_HOME", root)
        .stdout(Stdio::from(File::create(&stdout).unwrap()))
        .stderr(Stdio::from(File::create(&stderr).unwrap()));
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy();
        if name.starts_with("GIT_") || name.starts_with("KAO_") {
            command.env_remove(key);
        }
    }
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null");
}

fn execute(root: &Path, command: &mut Command) -> (ExitStatus, String, String) {
    configure(root, command);
    let description = format!("{command:?}");
    let mut child = command.spawn().expect("spawn fixture command");
    finish(root, &mut child, &description)
}

fn finish(
    root: &Path,
    child: &mut std::process::Child,
    description: &str,
) -> (ExitStatus, String, String) {
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!(
                "command timed out: {description}; fixture: {}",
                root.display()
            );
        }
        thread::sleep(Duration::from_millis(10));
    };
    let output = fs::read_to_string(root.join("stdout.log")).unwrap();
    let errors = fs::read_to_string(root.join("stderr.log")).unwrap();
    eprintln!("{description}\nstatus: {status}\nstdout: {output}\nstderr: {errors}");
    (status, output, errors)
}

#[cfg(target_os = "macos")]
pub fn read_pipe_chunk(reader: &mut std::io::PipeReader, limit: usize) -> Vec<u8> {
    use std::io::Read;
    use std::os::fd::AsRawFd;
    let mut descriptor = libc::pollfd {
        fd: reader.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    assert_eq!(
        unsafe { libc::poll(&mut descriptor, 1, 10_000) },
        1,
        "capture pipe timed out"
    );
    let mut bytes = vec![0; limit];
    let count = reader.read(&mut bytes).unwrap();
    bytes.truncate(count);
    bytes
}

#[cfg(target_os = "macos")]
pub struct RunningCapture {
    child: std::process::Child,
    logs: std::path::PathBuf,
}

#[cfg(target_os = "macos")]
impl RunningCapture {
    pub fn wait_for_output(&mut self, signal: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let output = fs::read_to_string(self.logs.join("stdout.log")).unwrap();
            if output.contains(signal) {
                return;
            }
            assert!(
                self.child.try_wait().unwrap().is_none(),
                "command exited before {signal:?}; logs: {}",
                self.logs.display()
            );
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {signal:?}; logs: {}",
                self.logs.display()
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    pub fn assert_waiting(&mut self, command_signal: &str) {
        self.wait_for_output("invoking Kao\n");
        let deadline = Instant::now() + Duration::from_millis(300);
        loop {
            let output = fs::read_to_string(self.logs.join("stdout.log")).unwrap();
            assert!(
                !output.contains(command_signal),
                "second command started while the first held the lock"
            );
            assert!(
                self.child.try_wait().unwrap().is_none(),
                "second Kao invocation must wait for the lock"
            );
            if Instant::now() >= deadline {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    pub fn release(&mut self) {
        use std::io::Write;
        self.child
            .stdin
            .take()
            .unwrap()
            .write_all(b"continue\n")
            .unwrap();
    }

    pub fn finish_stream(self, capture: Vec<u8>) -> (ExitStatus, String, String, Vec<u8>) {
        fs::write(self.logs.join("capture.tar"), capture).unwrap();
        self.finish()
    }

    pub fn finish(mut self) -> (ExitStatus, String, String, Vec<u8>) {
        let (status, stdout, stderr) =
            finish(&self.logs, &mut self.child, "concurrent Kao capture");
        let capture = fs::read(self.logs.join("capture.tar")).unwrap();
        (status, stdout, stderr, capture)
    }
}

#[cfg(target_os = "macos")]
impl Drop for RunningCapture {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            unsafe { libc::kill(-(self.child.id() as i32), libc::SIGKILL) };
            let _ = self.child.wait();
        }
    }
}

pub struct Repository {
    root: std::path::PathBuf,
    path: std::path::PathBuf,
}

impl Repository {
    pub fn new(name: &str) -> Self {
        let root = tempfile::tempdir().unwrap().keep();
        eprintln!("fixture retained on failure: {}", root.display());
        let path = root.join(name);
        fs::create_dir(&path).unwrap();
        let repo = Self { root, path };
        repo.git(&["init", "--initial-branch=main"]);
        repo
    }

    pub fn write(&self, path: &str, content: &str) {
        fs::write(self.path.join(path), content).unwrap();
    }

    pub fn read(&self, path: &str) -> String {
        fs::read_to_string(self.path.join(path)).unwrap()
    }

    pub fn commit(&self, message: &str) {
        self.git(&["add", "."]);
        self.git(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-m",
            message,
        ]);
    }

    pub fn kao(&self, args: &[&str]) -> String {
        run(
            &self.root,
            Command::new(env!("CARGO_BIN_EXE_kao"))
                .current_dir(&self.path)
                .args(args),
        )
        .1
    }

    #[cfg(target_os = "macos")]
    pub fn capture(&self, args: &[&str]) -> (ExitStatus, String, String, Vec<u8>) {
        let capture = self.root.join("capture.tar");
        let (status, stdout, stderr) = execute(
            &self.root,
            Command::new("bash")
                .current_dir(&self.path)
                .args(["-c", "exec 3>\"$1\"; shift; exec \"$@\"", "capture"])
                .arg(&capture)
                .arg(env!("CARGO_BIN_EXE_kao"))
                .args(args),
        );
        (status, stdout, stderr, fs::read(capture).unwrap())
    }

    #[cfg(target_os = "macos")]
    pub fn spawn_capture(&self, name: &str, args: &[&str]) -> RunningCapture {
        use std::os::unix::process::CommandExt;
        let logs = self.root.join(name);
        fs::create_dir(&logs).unwrap();
        let mut command = Command::new("bash");
        command
            .current_dir(&self.path)
            .args([
                "-c",
                "exec 3>\"$1\"; shift; printf 'invoking Kao\\n'; exec \"$@\"",
                "capture",
            ])
            .arg(logs.join("capture.tar"))
            .arg(env!("CARGO_BIN_EXE_kao"))
            .args(args)
            .stdin(Stdio::piped())
            .process_group(0);
        configure(&logs, &mut command);
        let child = command.spawn().expect("spawn concurrent Kao invocation");
        RunningCapture { child, logs }
    }

    #[cfg(target_os = "macos")]
    pub fn spawn_pipe_capture(
        &self,
        name: &str,
        args: &[&str],
    ) -> (RunningCapture, std::io::PipeReader) {
        use std::os::fd::AsRawFd;
        use std::os::unix::process::CommandExt;

        let logs = self.root.join(name);
        fs::create_dir(&logs).unwrap();
        let (reader, writer) = std::io::pipe().unwrap();
        let descriptor = writer.as_raw_fd();
        let mut command = Command::new(env!("CARGO_BIN_EXE_kao"));
        command
            .current_dir(&self.path)
            .args(args)
            .stdin(Stdio::null())
            .process_group(0);
        configure(&logs, &mut command);
        unsafe {
            command.pre_exec(move || {
                if libc::dup2(descriptor, 3) < 0 || libc::fcntl(3, libc::F_SETFD, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if descriptor != 3 {
                    libc::close(descriptor);
                }
                Ok(())
            });
        }
        let child = command.spawn().expect("spawn Kao with capture pipe");
        drop(writer);
        (RunningCapture { child, logs }, reader)
    }

    #[cfg(target_os = "macos")]
    pub fn canonical_path(&self) -> std::path::PathBuf {
        self.path.canonicalize().unwrap()
    }

    #[cfg(target_os = "macos")]
    pub fn reverse_patch(&self, patch: &[u8]) {
        let patch_path = self.root.join("changes.patch");
        fs::write(&patch_path, patch).unwrap();
        run(
            &self.root,
            Command::new("git")
                .current_dir(&self.path)
                .args(["apply", "--reverse"])
                .arg(patch_path),
        );
    }

    pub fn status(&self) -> String {
        self.git(&["status", "--porcelain"])
    }

    fn git(&self, args: &[&str]) -> String {
        run(
            &self.root,
            Command::new("git").current_dir(&self.path).args(args),
        )
        .1
    }
}

impl Drop for Repository {
    fn drop(&mut self) {
        if !thread::panicking() {
            fs::remove_dir_all(&self.root).unwrap();
        }
    }
}
