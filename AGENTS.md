# How to develop kao

Development in kao is driven by the integration tests. They exercise the CLI from the outside and cover real use cases in an isolated environment. Every feature starts with a new test or a change to an existing test.

- `tests/scenarios.rs`: one test per scenario.
- `tests/support/mod.rs`: shared helpers, such as `Repository` and `read_capture`. Prefer readable tests by extracting shared helpers here.

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

A change to the behavior of Kao updates README.md in the same commit. A change to capture performance updates BENCHMARKS.md.
