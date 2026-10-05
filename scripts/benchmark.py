import argparse
import hashlib
import json
import os
from pathlib import Path
import statistics
import subprocess
import tarfile
import tempfile
import time


parser = argparse.ArgumentParser()
parser.add_argument("--binary", default="target/release/kao")
parser.add_argument("--samples", type=int, default=12)
parser.add_argument("--files", type=int, default=1000)
parser.add_argument("--max-overhead-ms", type=float)
args = parser.parse_args()
binary = Path(args.binary).resolve()
assert args.samples >= 2
assert args.files >= 8


def run(command, root, **options):
    return subprocess.run(command, cwd=root, check=True, stdout=subprocess.DEVNULL,
                          stderr=subprocess.PIPE, **options)


def measure(root, script, wrapped, output, expected_changes):
    before = {
        f"src/file-{index:04}.txt": hashlib.sha256(
            (root / "src" / f"file-{index:04}.txt").read_bytes()).hexdigest()
        for index in range(expected_changes)
    } if wrapped else {}
    with output.open("wb") as capture:
        if capture.fileno() != 3:
            os.dup2(capture.fileno(), 3)
        started = time.perf_counter_ns()
        command = [str(binary), "run", "--", "bash", "-c", script] if wrapped else ["bash", "-c", script]
        run(command, root, pass_fds=(3,) if wrapped else ())
        elapsed = (time.perf_counter_ns() - started) / 1_000_000
    if wrapped:
        with tarfile.open(output) as archive:
            entries = archive.getnames()
            assert entries[-1] == "result.json"
            result = json.load(archive.extractfile("result.json"))
            assert result["capture"]["complete"] is True
            assert result["command"]["exit_code"] == 0
            for changed in result["files"]:
                assert before[changed["path"]] == changed["before_sha256"]
                contents = (root / changed["path"]).read_bytes()
                assert hashlib.sha256(contents).hexdigest() == changed["after_sha256"]
                if changed["after_blob"]:
                    assert archive.extractfile(changed["after_blob"]).read() == contents
            patch = archive.extractfile("changes.patch").read()
            assert bool(patch) == bool(result["files"])
        return elapsed, len(result["files"])
    return elapsed, None


def p95(values):
    return sorted(values)[max(0, (len(values) * 95 + 99) // 100 - 1)]


with tempfile.TemporaryDirectory(prefix="kao-benchmark-") as temporary:
    parent = Path(temporary)
    root = parent / "repository"
    root.mkdir()
    (root / "src").mkdir()
    (root / "node_modules").mkdir()
    (root / ".gitignore").write_text("node_modules/\n")
    payload = "baseline content\n" * 64
    for index in range(args.files):
        (root / "src" / f"file-{index:04}.txt").write_text(payload)
        (root / "node_modules" / f"package-{index:04}.js").write_text(payload)
    environment = os.environ.copy()
    for key in list(environment):
        if key.startswith(("GIT_", "KAO_")):
            del environment[key]
    environment.update(HOME=str(parent), XDG_CONFIG_HOME=str(parent),
                       GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL="/dev/null")
    os.environ.clear()
    os.environ.update(environment)
    run(["git", "init", "--initial-branch=main"], root)
    run(["git", "add", "."], root)
    run(["git", "-c", "user.name=Benchmark", "-c", "user.email=benchmark@example.com",
         "-c", "commit.gpgsign=false", "commit", "-m", "Benchmark fixture"], root)
    run(["git", "gc", "--quiet"], root)
    cases = [
        ("no-change", "true", 0),
        ("one-file", "printf 'changed\\n' >> src/file-0000.txt", 1),
        ("eight-files", "; ".join(f"printf 'changed\\n' >> src/file-{i:04}.txt" for i in range(8)), 8),
    ]
    report = {"tracked_files": args.files + 1, "ignored_files": args.files,
              "samples": args.samples, "cases": {}}
    cold, changes = measure(root, "true", True, parent / "capture.tar", 0)
    assert changes == 0
    report["first_capture_ms"] = round(cold, 3)
    for name, script, expected_changes in cases:
        for _ in range(2):
            _, changes = measure(root, script, True, parent / "capture.tar", expected_changes)
            assert changes == expected_changes
        baseline = []
        captured = []
        overhead = []
        for index in range(args.samples):
            order = [False, True] if index % 2 == 0 else [True, False]
            sample = {}
            for wrapped in order:
                elapsed, changes = measure(root, script, wrapped, parent / "capture.tar", expected_changes)
                if wrapped:
                    assert changes == expected_changes
                sample[wrapped] = elapsed
            baseline.append(sample[False])
            captured.append(sample[True])
            overhead.append(sample[True] - sample[False])
        report["cases"][name] = {
            "command_median_ms": round(statistics.median(baseline), 3),
            "kao_median_ms": round(statistics.median(captured), 3),
            "overhead_median_ms": round(statistics.median(overhead), 3),
            "overhead_p95_ms": round(p95(overhead), 3),
        }
    print(json.dumps(report, indent=2))
    if args.max_overhead_ms is not None:
        assert all(case["overhead_median_ms"] <= args.max_overhead_ms
                   for case in report["cases"].values()), "capture overhead exceeds the requested budget"
