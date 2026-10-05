use std::collections::VecDeque;
use std::ffi::OsString;
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const MINIMUM_GIT: (u32, u32) = (2, 41);
const STORE_LIMIT_BYTES: u64 = 256 * 1024 * 1024;
const STORE_CHECK_INTERVAL: u64 = 64;
const GITLINK: u32 = 0o160000;

pub struct Workspace {
    pub root: PathBuf,
    pub git_dir: PathBuf,
    pub common_dir: PathBuf,
    empty_tree: &'static str,
}

pub struct Tree(pub String);

pub struct Entry {
    pub mode: u32,
    pub bytes: Vec<u8>,
}

pub struct Change {
    pub path: String,
    pub before: Option<Entry>,
    pub after: Option<Entry>,
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

fn single_line(output: Output) -> Vec<u8> {
    let mut bytes = output.stdout;
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
    }
    bytes
}

pub fn check_git_version() -> Result<()> {
    let output = checked(Command::new("git").arg("version"))
        .map_err(|error| format!("Kao requires Git: {error}"))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let version = text.trim().trim_start_matches("git version ");
    let mut numbers = version
        .split(|character: char| !character.is_ascii_digit())
        .filter_map(|part| part.parse::<u32>().ok());
    let found = (numbers.next().unwrap_or(0), numbers.next().unwrap_or(0));
    if found < MINIMUM_GIT {
        return Err(format!(
            "Kao requires Git {}.{} or newer; found {version}",
            MINIMUM_GIT.0, MINIMUM_GIT.1
        )
        .into());
    }
    Ok(())
}

fn read_path_file(path: &Path) -> Result<Option<PathBuf>> {
    match fs::read(path) {
        Ok(mut bytes) => {
            while bytes.last().is_some_and(u8::is_ascii_whitespace) {
                bytes.pop();
            }
            let bytes = bytes.strip_prefix(b"gitdir: ").unwrap_or(&bytes).to_vec();
            let target = PathBuf::from(OsString::from_vec(bytes));
            Ok(Some(
                path.parent().ok_or("path file has no parent")?.join(target),
            ))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

impl Workspace {
    pub fn discover(cwd: &Path) -> Result<Self> {
        let output = single_line(checked(Command::new("git").current_dir(cwd).args([
            "rev-parse",
            "--show-object-format",
            "--show-toplevel",
        ]))?);
        let newline = output
            .iter()
            .position(|byte| *byte == b'\n')
            .ok_or("Kao requires a Git working tree")?;
        let (format, root) = (&output[..newline], &output[newline + 1..]);
        let empty_tree = match format {
            b"sha1" => "4b825dc642cb6eb9a060e54bf8d69288fbee4904",
            b"sha256" => "6ef19b41225c5369f1c104d45d8d85efa9b057b53b14b4b9b939dd74decc5321",
            other => {
                return Err(format!(
                    "unsupported object format: {}",
                    String::from_utf8_lossy(other)
                )
                .into());
            }
        };
        let root = PathBuf::from(OsString::from_vec(root.to_vec())).canonicalize()?;
        let dot_git = root.join(".git");
        let git_dir = if dot_git.is_dir() {
            dot_git
        } else {
            read_path_file(&dot_git)?.ok_or("Kao requires a .git directory or file")?
        }
        .canonicalize()?;
        let common_dir = read_path_file(&git_dir.join("commondir"))?
            .unwrap_or_else(|| git_dir.clone())
            .canonicalize()?;
        Ok(Self {
            root,
            git_dir,
            common_dir,
            empty_tree,
        })
    }

    fn store(&self) -> PathBuf {
        self.git_dir.join("kao")
    }

    fn git(&self) -> Result<Command> {
        let store = self.store();
        let empty_tree = self.empty_tree;
        let mut command = Command::new("git");
        command
            .current_dir(&self.root)
            .env("GIT_INDEX_FILE", store.join("index"))
            .env("GIT_OBJECT_DIRECTORY", store.join("objects"))
            .env(
                "GIT_ALTERNATE_OBJECT_DIRECTORIES",
                self.common_dir.join("objects"),
            )
            .env("GIT_OPTIONAL_LOCKS", "0")
            .arg(format!("--attr-source={empty_tree}"));
        for setting in [
            "core.autocrlf=false",
            "core.safecrlf=false",
            "core.attributesFile=/dev/null",
            "core.fsmonitor=false",
            "core.trustctime=true",
            "core.checkStat=default",
            "core.quotePath=true",
            "diff.noprefix=false",
        ] {
            command.args(["-c", setting]);
        }
        Ok(command)
    }

    pub fn bound_store(&self) -> Result<()> {
        let store = self.store();
        fs::create_dir_all(store.join("objects"))?;
        let counter = store.join("operation-count");
        let count = fs::read_to_string(&counter)
            .ok()
            .and_then(|text| text.trim().parse::<u64>().ok())
            .unwrap_or(0)
            + 1;
        fs::write(&counter, count.to_string())?;
        if count.is_multiple_of(STORE_CHECK_INTERVAL)
            && directory_size(&store.join("objects"))? > STORE_LIMIT_BYTES
        {
            self.reset_store()?;
        }
        Ok(())
    }

    fn reset_store(&self) -> Result<()> {
        match fs::remove_dir_all(self.store()) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    pub fn snapshot(&self) -> Result<Tree> {
        fs::create_dir_all(self.store().join("objects"))?;
        let tree = self
            .git()
            .and_then(|mut add| checked(add.args(["add", "-A"])))
            .and_then(|_| self.git())
            .and_then(|mut write| checked(write.arg("write-tree")))
            .map(single_line);
        match tree {
            Ok(tree) => Ok(Tree(String::from_utf8(tree)?)),
            Err(error) => {
                self.reset_store()?;
                check_git_version()?;
                Err(format!("snapshot failed: {error}").into())
            }
        }
    }

    pub fn changes(&self, before: &Tree, after: &Tree) -> Result<(Vec<Change>, Vec<u8>)> {
        let raw = checked(self.git()?.args([
            "diff-tree",
            "-r",
            "-z",
            "--raw",
            "--no-renames",
            &before.0,
            &after.0,
        ]))?
        .stdout;
        let mut records = raw.split(|byte| *byte == 0);
        let mut listed = Vec::new();
        while let Some(header) = records.next() {
            if header.is_empty() {
                continue;
            }
            let path = records.next().ok_or("malformed diff-tree output")?;
            let header = std::str::from_utf8(header)?.trim_start_matches(':');
            let fields: Vec<_> = header.split(' ').collect();
            let [old_mode, new_mode, old_id, new_id, ..] = fields[..] else {
                return Err("malformed diff-tree output".into());
            };
            let path =
                String::from_utf8(path.to_vec()).map_err(|_| "capture paths must be UTF-8")?;
            let old_mode = u32::from_str_radix(old_mode, 8)?;
            let new_mode = u32::from_str_radix(new_mode, 8)?;
            if old_mode == GITLINK || new_mode == GITLINK {
                return Err(format!(
                    "submodules and embedded repositories are not supported: {path}"
                )
                .into());
            }
            listed.push((
                path,
                old_mode,
                old_id.to_owned(),
                new_mode,
                new_id.to_owned(),
            ));
        }
        if listed.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }
        let mut ids = Vec::new();
        for (_, old_mode, old_id, new_mode, new_id) in &listed {
            if *old_mode != 0 {
                ids.push(old_id.clone());
            }
            if *new_mode != 0 {
                ids.push(new_id.clone());
            }
        }
        let mut blobs: VecDeque<_> = self.read_blobs(&ids)?.into();
        let mut changes = Vec::new();
        for (path, old_mode, _, new_mode, _) in listed {
            let before = (old_mode != 0).then(|| Entry {
                mode: old_mode,
                bytes: blobs.pop_front().unwrap_or_default(),
            });
            let after = (new_mode != 0).then(|| Entry {
                mode: new_mode,
                bytes: blobs.pop_front().unwrap_or_default(),
            });
            changes.push(Change {
                path,
                before,
                after,
            });
        }
        let patch = checked(self.git()?.args([
            "diff",
            "--binary",
            "--no-renames",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--no-relative",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            &before.0,
            &after.0,
        ]))?
        .stdout;
        Ok((changes, patch))
    }

    fn read_blobs(&self, ids: &[String]) -> Result<Vec<Vec<u8>>> {
        let mut child = self
            .git()?
            .args(["cat-file", "--batch"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let mut stdin = child.stdin.take().ok_or("missing cat-file input")?;
        let request: Vec<u8> = ids
            .iter()
            .flat_map(|id| format!("{id}\n").into_bytes())
            .collect();
        let writer = std::thread::spawn(move || stdin.write_all(&request));
        let mut reader = BufReader::new(child.stdout.take().ok_or("missing cat-file output")?);
        let mut blobs = Vec::new();
        for id in ids {
            let mut header = String::new();
            reader.read_line(&mut header)?;
            let size: usize = header
                .trim_end()
                .rsplit(' ')
                .next()
                .and_then(|size| size.parse().ok())
                .ok_or_else(|| format!("missing Git object {id}"))?;
            let mut bytes = vec![0; size + 1];
            reader.read_exact(&mut bytes)?;
            bytes.pop();
            blobs.push(bytes);
        }
        writer.join().map_err(|_| "cat-file writer failed")??;
        if !child.wait()?.success() {
            return Err("Git cat-file failed".into());
        }
        Ok(blobs)
    }

    pub fn disk_bytes(&self, path: &str) -> Result<Vec<u8>> {
        let path = self.root.join(path);
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            Ok(fs::read_link(&path)?.as_os_str().as_bytes().to_vec())
        } else {
            Ok(fs::read(&path)?)
        }
    }
}

fn directory_size(path: &Path) -> Result<u64> {
    let mut total = 0;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        total += if metadata.is_dir() {
            directory_size(&entry.path())?
        } else {
            metadata.len()
        };
    }
    Ok(total)
}
