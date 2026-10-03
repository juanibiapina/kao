# Kao

Kao (pronounced cow) is a command runner for agents.

> [!WARNING]
> Kao currently requires macOS and a Git working tree on an APFS filesystem.

## Usage

From a Git working tree:

```sh
kao run -- bash -c 'cargo fmt' 3>/tmp/capture.tar
```

Commands run in the current directory with unchanged stdout and stderr. Descriptor 3 receives a tar archive with `changes.patch`, after-content files in `blobs/`, and a final `result.json` manifest. A caller using a pipe must read it while Kao runs.

Kao also locks the repository so commands cannot run in parallel.

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
