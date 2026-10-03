# Capture benchmarks

Run on macOS with the repository and temporary directory on APFS:

```sh
cargo build --release --locked
python3 scripts/benchmark.py --files 1000 --samples 12
```

The fixture contains 1,001 tracked files and 1,000 ignored dependency files. Each data file starts at 1 KiB. Git objects are packed before measurement. Two warm-up captures precede 12 samples per case. Each sample compares the complete Kao invocation with the same Bash command run directly; the ordering alternates. Capture validation is outside the measured interval and checks the final manifest, changed-file count, content hashes, and after blobs.

## Baseline

Measured locally before optimization:

| Command | Kao median | Added overhead median | Added overhead p95 |
| --- | ---: | ---: | ---: |
| No changes | 160.110 ms | 151.823 ms | 156.527 ms |
| One changed file | 178.572 ms | 170.190 ms | 182.097 ms |
| Eight changed files | 185.351 ms | 176.791 ms | 182.817 ms |

Temporary stage timing identified snapshot cleanup at roughly 85 ms per invocation; directory cloning took roughly 15 ms. These results include process startup and cleanup, but no lock contention. They describe this fixture and machine, not a latency guarantee for arbitrary repositories.

## Scoped snapshots

Cloning only top-level entries containing tracked or nonignored files reduced median added overhead to 116.081 ms for no changes, 127.551 ms for one file, and 129.199 ms for eight files in a six-sample comparison. Git metadata and wholly ignored top-level directories are excluded from snapshots. Ignored descendants of a cloned directory can still be included. The reconstruction and error-recovery tests pass with this implementation.

Use `--max-overhead-ms NUMBER` to assert a median overhead budget for all three cases. The benchmark validates captures before checking the budget.
