# Kao

Kao (pronounced cow) is a command runner for agents. It runs a command in a Git working tree and reports the file changes that command made, as a Git patch.

> [!WARNING]
> Kao requires a Unix system (macOS or Linux) and Git 2.41 or newer.

## Usage

### Capture a command's changes

From a Git working tree:

```sh
kao run -- bash -c 'cargo fmt' 3>/tmp/capture.tar
```

The command runs in the current directory with unchanged stdin, stdout, and stderr. File descriptor 3 receives a tar archive containing:

- `changes.patch`: a Git binary patch from the files before the command to the files after it.
- `blobs/<sha256>`: after contents of modified text files.
- `result.json`: the manifest, written last. It lists each changed path with before and after SHA-256 hashes, plus the command outcome and whether the capture is complete.

A caller reading descriptor 3 through a pipe must drain it while Kao runs. Kao closes descriptor 3 in the command it runs.

Captured files are tracked files plus untracked files that `.gitignore` does not exclude. Kao records exact bytes: it ignores line-ending conversion and filters from `.gitattributes` and Git settings. Regular files, executable bits, and symlinks are supported. Changes involving submodules or embedded repositories make the capture incomplete.

Once Kao has written a complete archive on descriptor 3, it exits with the command's exit code, or 128 plus the signal number if a signal ended the command. This holds even when the capture is incomplete: `capture.complete` and `capture.error` in the manifest report the problem, and stderr names the operation's files, which Kao keeps.

Kao exits with 125 in two cases, and reports the reason on stderr:

- Kao never ran the command, for example when it is invoked incorrectly, outside a Git working tree, or the command cannot start.
- Kao ran the command but could not deliver the archive on descriptor 3. Stderr reports `capture output failed` and where the operation's files are kept.

### Cancel a command

Kao runs the command in its own process group. To cancel it, send SIGINT, SIGTERM, or SIGHUP to Kao's process ID only, never to the process group. Kao forwards the signal to the command's process group. If the group has not exited after 3 seconds, Kao sends it SIGKILL. Kao then captures the changes and writes the archive as usual; `command.signal` in the manifest reports the signal that ended the command.

After signalling Kao, wait for it to exit. Killing Kao with SIGKILL loses the capture: the changes stay on disk and the next capture includes them in its before contents. A signal that arrives while Kao waits for the lock ends Kao before the command runs. A signal that was ignored when Kao started, such as SIGHUP under `nohup`, stays ignored by Kao and the command.

When Kao is the foreground job of a terminal, it gives the terminal to the command while it runs, so the command can read it and Ctrl-C reaches the command.

### Run a command under the lock without capture

```sh
kao lock -- deltoids edit <trace-id>
```

`kao lock` waits for the same lock as `kao run`, runs the command, and returns its exit code. It does not capture changes; use it for tools that record their own changes, such as the Deltoids edit and write tools. A change made under `kao lock` never appears in a `kao run` capture.

### Locking

`kao run` and `kao lock` hold one lock per repository (`.git/kao.lock`, shared by linked worktrees) while the command runs and until the capture is written. Concurrent invocations wait, and long-running commands block other Kao invocations. Editors and processes started outside Kao do not take the lock; their writes during a `kao run` command appear in that command's capture.

## How it works

Kao keeps a private Git index and object directory in `.git/kao/`, or in the worktree's Git directory for a linked worktree. Before the command, Kao stages the working tree into that private index and writes a tree. After the command, it stages again and diffs the index against the before tree. Your index, branches, and commits are never changed.

The first capture in a working tree reads every file. Later captures only check file metadata and reread files that changed. Kao discards the private store when its objects exceed 256 MiB; the next capture rebuilds it.

Each operation's files live in `.git/kao/operations/` while Kao runs and are removed on success.

## Development

Build and run:

```sh
cargo build --locked
cargo run --locked -- run -- bash -c 'printf "hello\\n"' 3>/tmp/capture.tar
```

Run the same checks as CI:

```sh
cargo fmt --all -- --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

See [capture benchmarks](BENCHMARKS.md) for repeatable speed tests and measured overhead.
