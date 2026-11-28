"""Which tests of a Rust test binary keep a given share of the library lines the whole binary runs: each test is
run alone under coverage instrumentation, then a greedy cover picks the tests that add the most lines per second.

    python scripts/test_cover/cover.py --crate oxoffice-docx --test docx --target 0.99 --keep-module render \
        --keep-module corpus --keep-should-panic --target-dir ../target-cover --out docx-keep.txt

Needs the `llvm-tools` component of the pinned toolchain (`rustup component add llvm-tools`). Writes the keep list (one `module::test` per line, for prune.py) and, beside it, `<out>.lost.txt`: the library lines
(crates/*/src, the Python binding crates apart) the kept tests no longer run, counted by file. Each test runs in its
own process, so a cache shared by a module's tests (a render of the whole corpus) counts once per test:
`--keep-module` keeps such a module whole instead of weighing it, and the corpus checks (every part saved byte for
byte, the manifest) are kept whole for what a line count cannot see; so is every known failure
(`--keep-should-panic`). The measure builds without debug assertions: code under `#[cfg(debug_assertions)]` is
outside it. The cover weighs tests by their measured time, so a second run picks a close but not always identical
list: the committed list is the one the cut used. Run with OXOFFICE_RENDER and the other variables of the CI's test
step.
"""

import argparse
import collections
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
IGNORE = r"(/\.cargo/|/rustc/|/tests/|oxoffice-[a-z]*-?py/|/target)"


def tools():
    run = lambda *a: subprocess.run(["rustc", *a], capture_output=True, text=True, cwd=ROOT).stdout
    host = re.search(r"^host: (\S+)", run("-vV"), re.M).group(1)
    return Path(run("--print", "sysroot").strip()) / "lib" / "rustlib" / host / "bin"


def build(args):
    env = dict(os.environ, RUSTFLAGS="-C instrument-coverage", CARGO_TARGET_DIR=str(args.target_dir),
               LLVM_PROFILE_FILE=os.devnull)
    out = subprocess.run(["cargo", "test", "--profile", "scaling", "--locked", "-p", args.crate, "--test", args.test,
                          "--no-run", "--message-format=json"], env=env, cwd=ROOT, capture_output=True, text=True,
                         check=True).stdout
    exes = [json.loads(line).get("executable") for line in out.splitlines() if line.startswith("{")]
    return next(e for e in exes if e and Path(e).name.startswith(f"{args.test}-"))


def run_one(exe, crate_dir, name, work, bin_dir):
    """The library lines one test runs, and its time."""
    raw = work / f"{abs(hash(name))}.profraw"
    env = dict(os.environ, LLVM_PROFILE_FILE=str(raw), RUST_MIN_STACK=os.environ.get("RUST_MIN_STACK", "16777216"))
    start = time.monotonic()
    proc = subprocess.run([exe, "--exact", name, "--test-threads", "2", "-q"], cwd=crate_dir, env=env,
                          capture_output=True)
    seconds = time.monotonic() - start
    if proc.returncode:
        raise SystemExit(f"{name} fails: {proc.stdout.decode()[-2000:]}")
    data = raw.with_suffix(".profdata")
    subprocess.run([bin_dir / "llvm-profdata", "merge", "-sparse", "-o", data, raw], check=True)
    lcov = subprocess.run([bin_dir / "llvm-cov", "export", "-format=lcov", f"-instr-profile={data}",
                           f"-ignore-filename-regex={IGNORE}", "-skip-functions", exe],
                          capture_output=True, text=True, check=True).stdout
    raw.unlink()
    data.unlink()
    lines, current = set(), None
    for line in lcov.splitlines():
        if line.startswith("SF:"):
            current = os.path.relpath(line[3:], ROOT)
        elif line.startswith("DA:"):
            number, count = line[3:].split(",")[:2]
            if count != "0":
                lines.add(f"{current}:{number}")
    return name, lines, seconds


def should_panic(folder):
    """`module::test` of every test that expects a panic: the known failures, each the reproduction of an open
    issue, which a line count cannot weigh."""
    sys.path.insert(0, str(Path(__file__).parent))
    import rust_items
    found = set()
    for path in folder.glob("*.rs"):
        text = path.read_text()
        for name, (start, _) in rust_items.test_functions(text).items():
            if "should_panic" in text[start:text.index(f"fn {name}", start)]:
                found.add(f"{path.stem}::{name}")
    return found


def cover(sets, seconds, target, forced):
    """Greedy: the forced tests (whole modules by name, or `module::test`), then the test adding the most lines per
    second until `target` of the lines run."""
    total = set().union(*sets.values())
    keep = {name for name in sets if name in forced or name.split("::")[0] in forced}
    covered = set().union(*(sets[name] for name in keep)) if keep else set()
    while len(covered) < target * len(total):
        name = max((n for n in sets if n not in keep),
                   key=lambda n: len(sets[n] - covered) / max(seconds[n], 0.01))
        keep.add(name)
        covered |= sets[name]
    return keep, total, covered


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--crate", required=True)
    ap.add_argument("--test", required=True, help="the integration test binary")
    ap.add_argument("--target", type=float, default=0.99, help="share of the binary's lines to keep")
    ap.add_argument("--keep-module", action="append", default=[], help="a module kept whole")
    ap.add_argument("--keep-should-panic", action="store_true", help="keep every test that expects a panic")
    ap.add_argument("--target-dir", type=Path, required=True, help="a target folder of its own")
    ap.add_argument("--jobs", type=int, default=os.cpu_count())
    ap.add_argument("--out", type=Path, required=True)
    args = ap.parse_args()
    args.target_dir, args.out = args.target_dir.resolve(), args.out.resolve()
    exe = build(args)
    crate_dir = ROOT / "crates" / args.crate
    listed = subprocess.run([exe, "--list"], cwd=crate_dir, capture_output=True, text=True,
                            env=dict(os.environ, LLVM_PROFILE_FILE=os.devnull)).stdout
    names = [line[:-len(": test")] for line in listed.splitlines() if line.endswith(": test")]
    work = Path(tempfile.mkdtemp(prefix="cover-"))
    bin_dir = tools()
    with ThreadPoolExecutor(args.jobs) as pool:
        results = list(pool.map(lambda n: run_one(exe, crate_dir, n, work, bin_dir), names))
    shutil.rmtree(work)
    sets = {name: lines for name, lines, _ in results}
    seconds = {name: s for name, _, s in results}
    forced = set(args.keep_module)
    if args.keep_should_panic:
        forced |= should_panic(crate_dir / "tests" / args.test)
    keep, total, covered = cover(sets, seconds, args.target, forced)
    args.out.write_text("".join(f"{name}\n" for name in sorted(keep)))
    lost = collections.Counter(key.rsplit(":", 1)[0] for key in total - covered)
    Path(f"{args.out}.lost.txt").write_text("".join(f"{count:6d} {path}\n" for path, count in lost.most_common()))
    print(f"{len(names)} tests run {len(total)} library lines; {len(keep)} tests keep {len(covered)} "
          f"({len(covered) / len(total):.2%}); {sum(seconds.values()):.0f} s -> "
          f"{sum(seconds[n] for n in keep):.0f} s, one test per process")


if __name__ == "__main__":
    main()
