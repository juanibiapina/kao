# Kao

Kao (pronounced cow) is a copy on write sandboxed command runner.

The repository currently contains the default Cargo binary scaffold. Running it prints `Hello, world!`; command-runner behavior is not implemented yet.

## Development

Build and run:

```sh
cargo build --locked
cargo run --locked
```

Run the same checks as CI:

```sh
cargo fmt --all -- --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```
