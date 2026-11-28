"""Wall and CPU seconds of every Rust test run alone in its own process (the shared caches of a binary, a render of
the corpus for instance, are paid by each test: read a module that shares one as a block).

    python scripts/test_cover/rust_cpu.py --target-dir ../target-test-slim --out cpu.json [--jobs 8]

CPU (user + system) is what a CI runner with few cores waits for: a test that fans out over every core, as the
equivalent-writings walks do, costs its CPU time there, not its wall time here.

Requirements: Linux (GNU `/usr/bin/time -f`).
"""

import argparse
import json
import os
import re
import subprocess
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

from rust_time import ROOT, executables


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--target-dir", type=Path, required=True)
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("--jobs", type=int, default=8)
    ap.add_argument("--render", default="subset")
    args = ap.parse_args()
    env = dict(os.environ, RUST_MIN_STACK="8388608", OXOFFICE_RENDER=args.render, CI="true")
    todo = []
    for crate, name, exe in executables(args.target_dir.resolve()):
        listed = subprocess.run([exe, "--list"], cwd=ROOT / "crates" / crate, capture_output=True, text=True,
                                env=env).stdout
        todo += [(crate, name, exe, line[:-len(": test")]) for line in listed.splitlines() if line.endswith(": test")]

    def one(unit):
        crate, name, exe, test = unit
        proc = subprocess.run(["/usr/bin/time", "-f", "TIME %e %U %S", exe, "--exact", test, "--test-threads", "1",
                               "-q"], cwd=ROOT / "crates" / crate, env=env, capture_output=True, text=True)
        m = re.search(r"TIME ([\d.]+) ([\d.]+) ([\d.]+)", proc.stderr)
        wall, user, system = (float(x) for x in m.groups())
        return f"{crate}/{name}::{test}", {"wall": wall, "cpu": user + system, "ok": proc.returncode == 0}

    with ThreadPoolExecutor(args.jobs) as pool:
        results = dict(pool.map(one, todo))
    args.out.write_text(json.dumps(results, indent=1))
    print(f"{len(results)} tests, wall {sum(r['wall'] for r in results.values()):.0f} s, "
          f"cpu {sum(r['cpu'] for r in results.values()):.0f} s")


if __name__ == "__main__":
    main()
