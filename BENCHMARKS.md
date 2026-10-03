# Capture benchmarks

Run on macOS with the repository and temporary directory on APFS:

```sh
cargo build --release --locked
python3 scripts/benchmark.py --files 1000 --samples 24
python3 scripts/benchmark.py --files 8 --samples 24
```

Measurements below were collected on a MacBookPro18,3, arm64, macOS 27.0.1, with Rust 1.99.0. They include process startup, output, and snapshot cleanup, but no lock contention.

## Method

The larger fixture contains 1,001 tracked files and 1,000 ignored dependency files. Each data file starts at 1 KiB. Git objects are packed before measurement. Two warm-up captures precede the requested samples. Each sample compares the complete Kao invocation with the same Bash command run directly; the ordering alternates.

Validation is outside the measured interval. It checks the final manifest, command outcome, changed-file count, before and after content hashes, and after blobs. Results describe this fixture and machine, not a latency guarantee for arbitrary repositories.

## Results

Baseline measurements used 12 samples per case; final measurements used 24.

| Command | Initial added overhead median | Final added overhead median | Final added overhead p95 |
| --- | ---: | ---: | ---: |
| No changes | 151.823 ms | 97.951 ms | 101.326 ms |
| One changed file | 170.190 ms | 106.815 ms | 112.564 ms |
| Eight changed files | 176.791 ms | 107.363 ms | 119.150 ms |

Median overhead fell by 35–39%. Complete final Kao invocations had medians of 105.853 ms, 115.294 ms, and 116.563 ms respectively.

For the smaller fixture, with nine tracked files and eight ignored files:

| Command | Added overhead median | Added overhead p95 |
| --- | ---: | ---: |
| No changes | 46.119 ms | 68.274 ms |
| One changed file | 49.057 ms | 56.057 ms |
| Eight changed files | 51.991 ms | 56.918 ms |

## Optimizations and limits

Initial stage timing identified snapshot cleanup at roughly 85 ms per invocation and directory cloning at roughly 15 ms. Switching to macOS native recursive removal did not improve latency; that experiment was reverted.

Snapshots now clone only top-level entries containing tracked or nonignored files. Git metadata and wholly ignored top-level directories are excluded. Ignored descendants of a cloned directory can still be included. Gitoxide discovers repositories in process. Known candidate paths reuse the initial scope, and empty patches avoid a Git diff subprocess. The CLI, FD 3 archive, and manifest format remain unchanged.

Snapshot creation still enumerates scoped file names, and cloning and deleting directory entries still cost time. The provisional 10 ms added-overhead target is not met. Further reductions would require addressing these costs and watcher completion latency while preserving capture correctness.

Use `--max-overhead-ms NUMBER` to assert a median overhead budget for all three cases. The benchmark validates captures before checking the budget.
