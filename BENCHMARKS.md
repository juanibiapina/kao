# Capture benchmarks

Run on macOS:

```sh
cargo build --release --locked
python3 scripts/benchmark.py --files 1000 --samples 24
python3 scripts/benchmark.py --files 20000 --samples 24
```

Measurements below were collected on a MacBookPro18,3, arm64, macOS 27.0.1, with Rust 1.99.0 and Git 2.56.0. They include process startup and archive output, but no lock contention.

## Method

The fixture contains `--files` tracked 1 KiB files plus the same number of ignored dependency files. Git objects are packed before measurement. The first capture is measured separately because it builds Kao's private index. Two warm-up captures per case follow, then the requested samples. Each sample compares the complete Kao invocation with the same Bash command run directly; the order alternates.

Validation runs outside the measured interval. It checks the final manifest, command outcome, changed-file count, before and after hashes, and after blobs. Results describe this fixture and machine. They are not a latency guarantee for arbitrary repositories.

## Results

Added overhead, median and 95th percentile:

| Tracked files | No changes | One changed file | Eight changed files | First capture |
| ---: | ---: | ---: | ---: | ---: |
| 1,001 | 61.7 ms (p95 68.3 ms) | 77.1 ms (p95 85.8 ms) | 81.3 ms (p95 105.1 ms) | 301.7 ms |
| 20,001 | 132.6 ms (p95 166.7 ms) | 161.6 ms (p95 181.9 ms) | 168.0 ms (p95 277.9 ms) | 1,187.1 ms |

The previous design cloned the working tree with APFS for every capture. It added 97.7 ms with no changes at 1,001 files, and a no-change command took 976 ms in total at 20,000 files.

Repeated runs varied by up to 10 ms when other work loaded the machine.

## Where the time goes

A capture starts five Git processes when nothing changes and six when files change. On this machine, each Git process costs about 7 ms to start. At 20,000 files, each `git add -A` takes about 47 ms because Git checks every file's metadata. Kao runs it twice per capture, so it dominates the cost. Git's untracked cache did not reduce it in measurement. Git's filesystem monitor (`core.fsmonitor`) is the next candidate; Kao currently disables it.

Use `--max-overhead-ms NUMBER` to assert a median overhead budget for all three cases. The benchmark validates captures before checking the budget.
