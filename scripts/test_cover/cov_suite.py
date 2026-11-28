"""The library lines the whole Rust suite runs, per crate: one instrumented run of every test binary (as CI runs them),
one merged profile per binary, the union of their lines. Compare two runs (before and after a cut) with `--compare`.

    python scripts/test_cover/cov_suite.py --target-dir ../target-test-slim-cov --out before.pkl
    python scripts/test_cover/cov_suite.py --target-dir ../target-test-slim-cov --out after.pkl
    python scripts/test_cover/cov_suite.py --compare before.pkl after.pkl [--lost]

The test code (`#[cfg(test)]` items and the files they declare), the Python binding crates and the two test support
crates are left out, as in measure.py. `--lost` lists the library lines the second run no longer runs, by file.

Requirements: Linux (or macOS for the scripts that do not call GNU `time`), the `llvm-tools` component of the pinned
toolchain (`rustup component add llvm-tools`), Python 3.10 or later. The instrumented builds use the `scaling` profile
(the folder `<target>/scaling` is read for the `oxoffice` commands the CLI tests spawn: rename it here with the
profile) and a target folder of their own.
"""

import argparse
import collections
import os
import pickle
import shutil
import subprocess
import sys
import tempfile
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from measure import IGNORE, ROOT, Table, tools  # noqa: E402
from rust_time import executables  # noqa: E402


def run(args):
    target = args.target_dir.resolve()
    units = executables(target, rustflags="-C instrument-coverage")
    bin_dir = tools()
    extra = sorted(str(p) for p in (target / "scaling").iterdir() if p.is_file() and p.name in
                   ("oxoffice", "oxoffice-docx", "oxoffice-pptx", "oxoffice-xlsx"))
    work = Path(tempfile.mkdtemp(prefix="cov-", dir=os.environ.get("MEASURE_TMP")))
    env = dict(os.environ, RUST_MIN_STACK="8388608", OXOFFICE_RENDER=args.render, CI="true")
    table = Table()

    def one(unit):
        crate, name, exe = unit
        folder = work / f"{crate}-{name}"
        folder.mkdir()
        proc = subprocess.run([exe, "-q"], cwd=ROOT / "crates" / crate, capture_output=True, text=True,
                              env=dict(env, LLVM_PROFILE_FILE=str(folder / "%p-%m.profraw")))
        raws = list(folder.glob("*.profraw"))
        if not raws:
            return unit, None, proc.returncode
        data = folder / "all.profdata"
        subprocess.run([bin_dir / "llvm-profdata", "merge", "-sparse", "-o", data, *raws], check=True)
        objects = [exe] + (extra if crate == "oxoffice-cli" else [])
        cmd = [bin_dir / "llvm-cov", "export", "-format=lcov", f"-instr-profile={data}",
               f"-ignore-filename-regex={IGNORE}", "-skip-functions", objects[0]]
        for obj in objects[1:]:
            cmd += ["-object", obj]
        lcov = subprocess.run(cmd, capture_output=True, text=True, check=True).stdout
        shutil.rmtree(folder)
        return unit, table.bits(lcov), proc.returncode

    covered, failed = 0, []
    with ThreadPoolExecutor(args.jobs) as pool:
        for (crate, name, _), bits, code in pool.map(one, units):
            if bits is not None:
                covered |= bits
            if code:
                failed.append(f"{crate}/{name}")
    shutil.rmtree(work, ignore_errors=True)
    args.out.write_bytes(pickle.dumps({"lines": table.names, "covered": covered, "total": table.total,
                                       "failed": failed}))
    report(table.names, covered, table.total)
    if failed:
        print("failing binaries:", failed)


def report(names, covered, total, other=None):
    per = collections.defaultdict(lambda: [0, 0])
    for i, (path, _) in enumerate(names):
        crate = Path(path).parts[1]
        per[crate][0] += (total >> i) & 1
        per[crate][1] += (covered >> i) & 1
    lines, hit = sum(v[0] for v in per.values()), sum(v[1] for v in per.values())
    for crate, (n, h) in sorted(per.items()):
        print(f"{crate:28s} {h:7d} / {n:7d}  {h / n:7.2%}")
    print(f"{'all':28s} {hit:7d} / {lines:7d}  {hit / lines:7.2%}")


def compare(args):
    a, b = (pickle.loads(Path(p).read_bytes()) for p in args.compare)
    # one numbering: by (file, line)
    index = {k: i for i, k in enumerate(a["lines"])}
    per = collections.defaultdict(lambda: [0, 0, 0, 0])
    lost = collections.Counter()
    covered_b = {k for i, k in enumerate(b["lines"]) if (b["covered"] >> i) & 1}
    total_a = {k for i, k in enumerate(a["lines"]) if (a["total"] >> i) & 1}
    covered_a = {k for i, k in enumerate(a["lines"]) if (a["covered"] >> i) & 1}
    for key in total_a:
        crate = Path(key[0]).parts[1]
        per[crate][0] += 1
        per[crate][1] += key in covered_a
        per[crate][2] += key in covered_b
        if key in covered_a and key not in covered_b:
            lost[key[0]] += 1
    n = h1 = h2 = 0
    for crate, (t, c1, c2, _) in sorted(per.items()):
        n, h1, h2 = n + t, h1 + c1, h2 + c2
        print(f"{crate:28s} {c1 / t:7.2%} -> {c2 / t:7.2%}  ({c2 - c1:+d} lines of {t})")
    print(f"{'all':28s} {h1 / n:7.2%} -> {h2 / n:7.2%}  ({h2 - h1:+d} lines of {n})")
    if args.lost:
        for path, count in lost.most_common():
            print(f"{count:6d} {path}")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--target-dir", type=Path)
    ap.add_argument("--out", type=Path)
    ap.add_argument("--jobs", type=int, default=4)
    ap.add_argument("--render", default="subset")
    ap.add_argument("--compare", nargs=2)
    ap.add_argument("--lost", action="store_true")
    args = ap.parse_args()
    compare(args) if args.compare else run(args)


if __name__ == "__main__":
    main()
