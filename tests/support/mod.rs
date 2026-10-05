use std::fs::{self, File};
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};
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
pub type Rebuilt = std::collections::BTreeMap<String, Option<(String, Vec<u8>)>>;

pub fn rebuild_before(
    files: &std::collections::BTreeMap<String, Vec<u8>>,
    result: &serde_json::Value,
) -> Rebuilt {
    use std::os::unix::fs::PermissionsExt;

    let scratch = tempfile::tempdir().unwrap();
    let tree = scratch.path().join("tree");
    fs::create_dir(&tree).unwrap();
    run(
        scratch.path(),
        Command::new("git").current_dir(&tree).args(["init", "-q"]),
    );
    let patch = scratch.path().join("changes.patch");
    fs::write(&patch, &files["changes.patch"]).unwrap();
    let entries = result["files"].as_array().unwrap();
    for entry in entries {
        if let Some(blob) = entry["after_blob"].as_str() {
            let path = tree.join(entry["path"].as_str().unwrap());
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, &files[blob]).unwrap();
            let mode = if entry["after_mode"] == "100755" {
                0o755
            } else {
                0o644
            };
            fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
        }
    }
    let mut rebuilt = Rebuilt::new();
    for entry in entries {
        if entry["after_blob"].is_null() && !entry["after_sha256"].is_null() {
            continue;
        }
        let path = entry["path"].as_str().unwrap();
        let escaped: String = path
            .chars()
            .flat_map(|character| {
                let escape = "*?[\\".contains(character).then_some('\\');
                escape.into_iter().chain([character])
            })
            .collect();
        let (_, _, errors) = run(
            scratch.path(),
            Command::new("git")
                .current_dir(&tree)
                .args(["apply", "-R", "--binary", "--whitespace=nowarn"])
                .arg(format!("--include={escaped}"))
                .arg(&patch),
        );
        assert_eq!(errors, "", "reverse patch for {path} must apply cleanly");
        let file = tree.join(path);
        let before = match fs::symlink_metadata(&file) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => panic!("read rebuilt {path}: {error}"),
            Ok(metadata) if metadata.file_type().is_symlink() => {
                use std::os::unix::ffi::OsStrExt;
                let target = fs::read_link(&file).unwrap();
                Some(("120000".to_owned(), target.as_os_str().as_bytes().to_vec()))
            }
            Ok(metadata) => {
                let mode = if metadata.permissions().mode() & 0o111 != 0 {
                    "100755"
                } else {
                    "100644"
                };
                Some((mode.to_owned(), fs::read(&file).unwrap()))
            }
        };
        rebuilt.insert(path.to_owned(), before);
    }
    rebuilt
}

pub fn patch_modes(
    patch: &[u8],
) -> std::collections::BTreeMap<String, (Option<String>, Option<String>)> {
    let mut modes = std::collections::BTreeMap::new();
    let mut current: Option<String> = None;
    for line in String::from_utf8_lossy(patch).lines() {
        if let Some(paths) = line.strip_prefix("diff --git a/") {
            let path = paths.split(" b/").next().unwrap().to_owned();
            modes.entry(path.clone()).or_insert((None, None));
            current = Some(path);
            continue;
        }
        let Some(path) = &current else { continue };
        if ["---", "@@", "GIT binary patch", "Binary files"]
            .iter()
            .any(|marker| line.starts_with(marker))
        {
            current = None;
            continue;
        }
        let (before, after): &mut (Option<String>, Option<String>) = modes.get_mut(path).unwrap();
        let words: Vec<_> = line.split(' ').collect();
        match words[..] {
            ["old", "mode", mode] | ["deleted", "file", "mode", mode] => {
                *before = Some(mode.to_owned())
            }
            ["new", "mode", mode] | ["new", "file", "mode", mode] => *after = Some(mode.to_owned()),
            ["index", _, mode] => {
                *before = Some(mode.to_owned());
                *after = Some(mode.to_owned());
            }
            _ => {}
        }
    }
    modes
}

pub fn retained_artifacts(stderr: &str) -> std::path::PathBuf {
    let location = stderr
        .split("artifacts retained at ")
        .nth(1)
        .expect("Kao failures must report retained artifacts")
        .trim();
    let path = std::path::PathBuf::from(location);
    assert!(
        path.is_dir(),
        "retained artifacts must exist: {}",
        path.display()
    );
    path
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
pub fn process_group_exists(group: i32) -> bool {
    let alive = unsafe { libc::kill(-group, 0) } == 0;
    alive || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}
pub struct RunningCapture {
    child: std::process::Child,
    logs: std::path::PathBuf,
}
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

    pub fn signal(&self, signal: i32) {
        assert_eq!(unsafe { libc::kill(self.child.id() as i32, signal) }, 0);
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
        Self::with_object_format(name, "sha1")
    }

    pub fn with_object_format(name: &str, format: &str) -> Self {
        let root = tempfile::tempdir().unwrap().keep();
        eprintln!("fixture retained on failure: {}", root.display());
        let path = root.join(name);
        fs::create_dir(&path).unwrap();
        let repo = Self { root, path };
        repo.git(&[
            "init",
            "--initial-branch=main",
            &format!("--object-format={format}"),
        ]);
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

    pub fn kao_outcome(&self, args: &[&str]) -> (ExitStatus, String, String) {
        let mut command = Command::new(env!("CARGO_BIN_EXE_kao"));
        command.current_dir(&self.path).args(args);
        execute(&self.root, &mut command)
    }
    pub fn kao_in(&self, cwd: &Path, args: &[&str], input: &[u8]) -> (ExitStatus, String, String) {
        let input_path = self.root.join("stdin.txt");
        fs::write(&input_path, input).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_kao"));
        command
            .current_dir(cwd)
            .args(args)
            .stdin(Stdio::from(File::open(input_path).unwrap()));
        execute(&self.root, &mut command)
    }

    pub fn outside(&self) -> std::path::PathBuf {
        self.root.clone()
    }

    pub fn fake_git(&self, version: &str) -> std::path::PathBuf {
        let directory = self.root.join("fake-git");
        fs::create_dir_all(&directory).unwrap();
        let real = String::from_utf8(
            Command::new("sh")
                .args(["-c", "command -v git"])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();
        let script = directory.join("git");
        fs::write(
            &script,
            format!(
                "#!/bin/sh\nif [ \"$1\" = version ]; then echo 'git version {version}'; exit 0; fi\ncase \"$1\" in --attr-source=*) echo \"unknown option: $1\" >&2; exit 129;; esac\nexec '{}' \"$@\"\n",
                real.trim()
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        directory
    }
    pub fn capture_with_unwritable_descriptor(
        &self,
        readonly: bool,
        args: &[&str],
    ) -> (ExitStatus, String, String) {
        let script = if readonly {
            "exec 3<\"$1\"; shift; exec \"$@\""
        } else {
            "exec 3>&-; shift; exec \"$@\""
        };
        execute(
            &self.root,
            Command::new("bash")
                .current_dir(&self.path)
                .args(["-c", script, "capture"])
                .arg(self.path.join("duck.txt"))
                .arg(env!("CARGO_BIN_EXE_kao"))
                .args(args),
        )
    }
    pub fn capture(&self, args: &[&str]) -> (ExitStatus, String, String, Vec<u8>) {
        self.capture_from(&self.path, args)
    }
    pub fn capture_from(&self, cwd: &Path, args: &[&str]) -> (ExitStatus, String, String, Vec<u8>) {
        self.capture_with_path(cwd, None, args)
    }

    pub fn capture_with_path(
        &self,
        cwd: &Path,
        path_prefix: Option<&Path>,
        args: &[&str],
    ) -> (ExitStatus, String, String, Vec<u8>) {
        let capture = self.root.join("capture.tar");
        let mut command = Command::new("bash");
        command
            .current_dir(cwd)
            .args(["-c", "exec 3>\"$1\"; shift; exec \"$@\"", "capture"])
            .arg(&capture)
            .arg(env!("CARGO_BIN_EXE_kao"))
            .args(args);
        if let Some(prefix) = path_prefix {
            let path = std::env::var_os("PATH").unwrap_or_default();
            let mut paths = vec![prefix.to_path_buf()];
            paths.extend(std::env::split_paths(&path));
            command.env("PATH", std::env::join_paths(paths).unwrap());
        }
        let (status, stdout, stderr) = execute(&self.root, &mut command);
        (status, stdout, stderr, fs::read(capture).unwrap())
    }
    pub fn capture_in_terminal(&self, args: &[&str], input: &[u8]) -> (ExitStatus, Vec<u8>) {
        use std::io::{Read, Write};
        use std::os::fd::{AsRawFd, FromRawFd};
        use std::os::unix::process::CommandExt;

        let (mut controller, mut terminal) = (0, 0);
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut controller,
                    &mut terminal,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            },
            0,
            "open a pseudo-terminal"
        );
        let mut controller = unsafe { File::from_raw_fd(controller) };
        let capture_path = self.root.join("capture.tar");
        let capture = File::create(&capture_path).unwrap();
        let capture_descriptor = capture.as_raw_fd();
        let mut command = Command::new(env!("CARGO_BIN_EXE_kao"));
        command.current_dir(&self.path).args(args);
        configure(&self.root, &mut command);
        unsafe {
            command.pre_exec(move || {
                if libc::setsid() < 0 || libc::ioctl(terminal, libc::TIOCSCTTY as _, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                for target in 0..3 {
                    if libc::dup2(terminal, target) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                }
                if libc::dup2(capture_descriptor, 3) < 0 || libc::fcntl(3, libc::F_SETFD, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = command.spawn().expect("spawn Kao in a terminal");
        unsafe { libc::close(terminal) };
        drop(capture);
        let mut output = controller.try_clone().unwrap();
        thread::spawn(move || {
            let mut bytes = [0; 4096];
            while matches!(output.read(&mut bytes), Ok(1..)) {}
        });
        controller.write_all(input).unwrap();
        let (status, _, _) = finish(&self.root, &mut child, "Kao in a terminal");
        (status, fs::read(capture_path).unwrap())
    }
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
        unsafe {
            command.pre_exec(|| {
                for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
                    libc::signal(signal, libc::SIG_DFL);
                }
                Ok(())
            });
        }
        configure(&logs, &mut command);
        let child = command.spawn().expect("spawn concurrent Kao invocation");
        RunningCapture { child, logs }
    }
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
    pub fn add_worktree(&self, name: &str) -> std::path::PathBuf {
        let path = self.root.join(name);
        self.git(&["worktree", "add", "--detach", path.to_str().unwrap()]);
        path.canonicalize().unwrap()
    }
    pub fn operation_dirs(&self) -> Vec<std::path::PathBuf> {
        match fs::read_dir(self.path.join(".git/kao/operations")) {
            Ok(entries) => entries.map(|entry| entry.unwrap().path()).collect(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => panic!("read operations: {error}"),
        }
    }
    pub fn canonical_path(&self) -> std::path::PathBuf {
        self.path.canonicalize().unwrap()
    }
    pub fn reverse_patch(&self, patch: &[u8]) {
        self.reverse_patch_excluding(patch, &[]);
    }

    pub fn reverse_patch_excluding(&self, patch: &[u8], excluded: &[&str]) {
        let patch_path = self.root.join("changes.patch");
        fs::write(&patch_path, patch).unwrap();
        run(
            &self.root,
            Command::new("git")
                .current_dir(&self.path)
                .args(["apply", "--reverse"])
                .args(excluded.iter().map(|path| format!("--exclude={path}")))
                .arg(patch_path),
        );
    }
    pub fn config(&self, key: &str, value: &str) {
        self.git(&["config", key, value]);
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
