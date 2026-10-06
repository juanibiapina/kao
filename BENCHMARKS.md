# Capture benchmarks

`scripts/benchmark.py` measures the time that `kao run` adds to a command.

## Run

```sh
cargo build --release --locked
python3 scripts/benchmark.py --files 1000 --samples 24
python3 scripts/benchmark.py --files 20000 --samples 24
```

The script takes these options:

- `--files NUMBER`: number of tracked files in the fixture. The default is 1000.
- `--samples NUMBER`: number of measured captures per case. The default is 12.
- `--binary PATH`: Kao binary to measure. The default is `target/release/kao`.
- `--max-overhead-ms NUMBER`: fail if the median overhead of a case is larger than this budget. The script checks the budget after it validates the captures.

## Method

The fixture contains `--files` tracked 1 KiB files and the same number of ignored dependency files. Git packs the objects before the measurement. The script measures the first capture separately, because it builds the private index of Kao. Each case then runs two warm-up captures and the requested samples. Each sample compares the complete Kao invocation with the same Bash command run directly. The order alternates.

Validation runs outside the measured interval. It checks the final manifest, the command outcome, the number of changed files, the before and after hashes, and the after blobs. The results describe this fixture and machine. They do not give a latency guarantee for other repositories.

## Results

These measurements are from a MacBookPro18,3, arm64, macOS 27.0.1, with Rust 1.99.0 and Git 2.56.0. They include process startup and archive output, but no lock contention.

Added overhead, median and 95th percentile:

| Tracked files | No changes | One changed file | Eight changed files | First capture |
| ---: | ---: | ---: | ---: | ---: |
| 1,001 | 61.7 ms (p95 68.3 ms) | 77.1 ms (p95 85.8 ms) | 81.3 ms (p95 105.1 ms) | 301.7 ms |
| 20,001 | 132.6 ms (p95 166.7 ms) | 161.6 ms (p95 181.9 ms) | 168.0 ms (p95 277.9 ms) | 1,187.1 ms |

Repeated runs varied by up to 10 ms when other work loaded the machine.

## Where the time goes

A capture starts five Git processes when nothing changes and six when files change. On this machine, each Git process takes about 7 ms to start. At 20,000 files, each `git add -A` takes about 47 ms, because Git checks the metadata of every file. Kao runs it twice per capture, so it is the largest cost. The Git untracked cache did not reduce it in measurement. Kao disables the Git filesystem monitor (`core.fsmonitor`).
