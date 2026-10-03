use std::fs::{self, File};
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

fn run(root: &Path, command: &mut Command) -> (ExitStatus, String, String) {
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
    let description = format!("{command:?}");
    let mut child = command.spawn().expect("spawn fixture command");
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
    let output = fs::read_to_string(stdout).unwrap();
    let errors = fs::read_to_string(stderr).unwrap();
    eprintln!("{description}\nstatus: {status}\nstdout: {output}\nstderr: {errors}");
    assert!(status.success(), "fixture: {}", root.display());
    (status, output, errors)
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
        let (status, stdout, stderr) = run(
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
