# Kao

> [!WARNING]
> Kao is an experiment. The main blocker is performance: each capture adds 60–170 ms to a command, mostly because Kao runs `git add -A` twice and Git checks the metadata of every file. See [BENCHMARKS.md](BENCHMARKS.md).

Kao (pronounced cow) tracks which files each command changes. It is made for agents that run commands in a Git repository.

Kao runs your command as usual, but it also returns data about every file that the command changed: which files, what they contained before, and what they contain after.

- The command runs as if you started it directly. It keeps its input, output, and terminal.
- Commands run sequentially and never interfere with each other.
- Kao adds about 60–80 ms to a command in a repository with 1,000 files, and about 130–170 ms with 20,000 files. See [BENCHMARKS.md](BENCHMARKS.md).

## Install

Install from source:

```sh
git clone https://github.com/juanibiapina/kao
cd kao
cargo install --locked --path .
```

## Run a command

From a Git working tree:

```sh
kao run -- bash -c 'cargo fmt' 3>/tmp/capture.tar
```

Kao writes a tar archive to file descriptor 3. Descriptor 3 must be open for writing, and the command does not inherit it. If you read it through a pipe, drain the pipe while Kao runs.

## The archive

Kao captures tracked files and untracked files that `.gitignore` does not exclude, as exact bytes. Line-ending conversion and Git filters do not apply. It supports regular files, executable bits, and symlinks. A change to a submodule, an embedded repository, or a path that is not UTF-8 makes the capture incomplete.

- `changes.patch`: a Git binary patch of all changes.
- `blobs/<sha256>`: the new contents of each changed text file. A text file is a regular file with no NUL byte in its first 8000 bytes.
- `result.json`: the manifest, written last.

| Manifest field | Meaning |
| --- | --- |
| `format_version` | `1` |
| `operation_id` | Name of the operation directory in `.git/kao/operations/` |
| `repository_root`, `cwd` | Absolute paths of the working tree and of the directory where the command ran |
| `command.exit_code` | Exit code, or `null` if a signal ended the command or it did not start |
| `command.signal` | Signal that ended the command, or `null` |
| `command.error` | Why the command did not start. Present only in that case. |
| `capture.complete`, `capture.error` | Whether Kao captured all changes, and why not. An incomplete capture has no files and an empty patch. |
| `files[].path` | Path relative to `repository_root` |
| `files[].before_mode`, `files[].after_mode` | Git mode, such as `100644`, `100755`, or `120000`. `null` where the file does not exist. |
| `files[].before_sha256`, `files[].after_sha256` | SHA-256 of the raw bytes, or of the target for a symlink. `null` where the file does not exist. |
| `files[].after_blob` | Blob with the new contents. `null` for deleted files, symlinks, and binary files, which are only in the patch. |

## Rebuild the before files

To get the files as they were before the command:

1. In an empty directory, write each after blob at its path with its `after_mode`.
2. For each path with an after blob or a `null` `after_mode`, run `git apply -R --binary --include=<path> changes.patch`. Put a `\` before each `*`, `?`, `[`, and `\` in the path.
3. For a deleted file, use `before_mode`. Some Git versions recreate it without the executable bit.

## Exit status

| Situation | Exit code | Archive |
| --- | --- | --- |
| The command ran | Its exit code, or 128 plus the signal number | Complete |
| The command ran, but the capture failed | Same as above | Complete, with `capture.complete` `false` |
| The command did not start | 125 | Complete, with `command.error` |
| Bad arguments, no Git working tree, descriptor 3 not writable, Git older than 2.41, or cancelled before the command started | 125 | None |
| Kao could not write the archive | 125 | None or partial |

Kao reports errors on stderr. When a capture fails after the command ran, stderr names the directory where Kao kept the operation files.

## Cancel a command

Send SIGINT, SIGTERM, or SIGHUP to the process ID of Kao, then wait for Kao to exit. Kao forwards the signal to the process group of the command, sends SIGKILL after 3 seconds, and then writes the archive as usual. Do not signal the process group yourself, and do not send SIGKILL to Kao. If Kao dies, the capture is lost and the next capture counts the changes as already there.

A signal that arrives while Kao waits for the lock ends Kao before the command runs. Signals that were ignored when Kao started stay ignored. If Kao runs in the foreground of a terminal, the command gets the terminal, so it can read input and receive Ctrl-C.

## Locking

Kao holds one lock per repository, in `.git/kao.lock`, shared by linked worktrees. `kao run` holds it until it writes the archive. Other Kao invocations wait.

`kao lock` runs a command under the lock without capturing, and exits with the exit code of the command. Use it for tools that record their own changes:

```sh
kao lock -- deltoids edit <trace-id>
```

Programs outside Kao, such as editors, do not take the lock. Their writes during a `kao run` command appear in its capture.
