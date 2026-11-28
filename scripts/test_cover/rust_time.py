"""Per-test time of the whole Rust suite, the way CI runs it (every test binary of `cargo test --workspace --lib
--test '*'`, one after the other, the tests of a binary in parallel).

    python scripts/test_cover/rust_time.py --target-dir ../target-test-slim --out times.json

Builds the test binaries with the `scaling` profile, runs each with libtest's JSON report (`--report-time`, which
needs RUSTC_BOOTSTRAP on a stable toolchain), and writes {"binaries": {name: seconds}, "tests": {"bin::test":
seconds}}.
"""

import argparse
import json
import os
import subprocess
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
PY = ["--exclude", "oxoffice-py", "--exclude", "oxoffice-docx-py", "--exclude", "oxoffice-pptx-py",
      "--exclude", "oxoffice-xlsx-py", "--exclude", "oxoffice-py-support"]


def executables(target_dir, extra_env=None, profile="scaling", rustflags=None):
    env = dict(os.environ, CARGO_TARGET_DIR=str(target_dir), **(extra_env or {}))
    if rustflags:
        env["RUSTFLAGS"] = rustflags
    out = subprocess.run(["cargo", "test", "--profile", profile, "--locked", "--workspace", *PY, "--lib",
                          "--test", "*", "--no-run", "--message-format=json"], env=env, cwd=ROOT,
                         capture_output=True, text=True, check=True).stdout
    found = []
    for line in out.splitlines():
        if not line.startswith("{"):
            continue
        item = json.loads(line)
        if item.get("reason") == "compiler-artifact" and item.get("executable") and item["profile"]["test"]:
            crate = item["package_id"].split("#")[0].rsplit("/", 1)[-1]
            found.append((crate, item["target"]["name"], item["executable"]))
    return found


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--target-dir", type=Path, required=True)
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("--render", default="subset")
    args = ap.parse_args()
    env = dict(os.environ, RUST_MIN_STACK="8388608", OXOFFICE_RENDER=args.render, RUSTC_BOOTSTRAP="1",
               CI="true")
    binaries, tests = {}, {}
    for crate, name, exe in executables(args.target_dir.resolve()):
        crate_dir = ROOT / "crates" / crate
        start = time.monotonic()
        proc = subprocess.run([exe, "--format", "json", "-Z", "unstable-options", "--report-time"], cwd=crate_dir,
                              env=env, capture_output=True, text=True)
        binaries[f"{crate}/{name}"] = time.monotonic() - start
        for line in proc.stdout.splitlines():
            if line.startswith("{"):
                item = json.loads(line)
                if item.get("type") == "test" and item.get("event") in ("ok", "failed", "ignored"):
                    tests[f"{crate}/{name}::{item['name']}"] = item.get("exec_time", 0.0)
                    if item["event"] == "failed":
                        print("FAILED", crate, name, item["name"])
        if proc.returncode:
            print("binary failed", crate, name, proc.stderr[-500:])
    args.out.write_text(json.dumps({"binaries": binaries, "tests": tests}, indent=1))
    print(f"{len(tests)} tests, {sum(binaries.values()):.0f} s over {len(binaries)} binaries")


if __name__ == "__main__":
    main()
