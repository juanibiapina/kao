# Kao

Kao is a planned Rust command runner that uses persistent copy-on-write clones to execute commands and merge changed files back into a Git repository.

The repository currently contains the default Cargo binary scaffold. Running it prints `Hello, world!`; command-runner behavior is not implemented yet.

## Development

Install Rust with [rustup](https://rustup.rs/). The repository pins the toolchain in `rust-toolchain.toml`; rustup installs it when you run Cargo.

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

See [PLAN.md](PLAN.md) for the implementation plan.
