"""The library lines each Rust test runs alone, and what it costs: the input of every cut of the suite.

    python scripts/test_cover/measure.py --target-dir ../target-test-slim-cov --out measure.pkl [--times times.json]

Builds every test binary CI runs (`cargo test --workspace --lib --test '*'`, profile `scaling`) with
`-C instrument-coverage`, then runs each test in its own process (`--exact`) and keeps the lines of the library
(`crates/*/src`, outside test code: `#[cfg(test)]` items and the files they declare, the Python binding crates and the
two test-support crates apart) that ran, as a bit set over one table of lines, with the test's time. A test of the
CLI crate also counts the lines its child `oxoffice` processes run (one profile file per process).

Writes a pickle {"lines": [name of each bit], "tests": {id: {"bits": int, "s": seconds, "crate": ..., "bin": ...}},
"total": bits of every instrumented library line}. `report.py` reads it.
"""

import argparse
import os
import pickle
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import rust_items  # noqa: E402
from rust_time import PY, ROOT  # noqa: E402

IGNORE = r"(/\.cargo/|/rustc/|/tests/|oxoffice-[a-z]*-?py/|oxoffice-render-test/|oxoffice-xml-test/|/target)"


def tools():
    run = lambda *a: subprocess.run(["rustc", *a], capture_output=True, text=True, cwd=ROOT).stdout
    host = re.search(r"^host: (\S+)", run("-vV"), re.M).group(1)
    return Path(run("--print", "sysroot").strip()) / "lib" / "rustlib" / host / "bin"


_test_code = {}


def test_code(path):
    """The line numbers of `path` that are test code: `#[cfg(test)]` items; True when the whole file is."""
    if path in _test_code:
        return _test_code[path]
    p = Path(path)
    ranges = set()
    text = p.read_text()
    mask = rust_items.code_mask(text)
    for m in re.finditer(r"#\[cfg\(test\)\]", mask):
        i = m.end()
        while True:                                   # further attributes
            n = re.match(r"\s*#\[", mask[i:])
            if not n:
                break
            i += n.end() - 2
            depth = 0
            while True:
                c = mask[i]
                depth += c == "["
                depth -= c == "]"
                i += 1
                if depth == 0:
                    break
        i += len(mask[i:]) - len(mask[i:].lstrip())
        end = rust_items.item_end(mask, i)
        first, last = text.count("\n", 0, m.start()) + 1, text.count("\n", 0, end) + 1
        ranges.update(range(first, last + 1))
        decl = re.match(r"(?:pub(?:\([a-z]+\))?\s+)?mod\s+(\w+)\s*;", mask[i:end])
        if decl:                                      # a test module in a file of its own
            for cand in (p.parent / f"{decl.group(1)}.rs", p.parent / p.stem / f"{decl.group(1)}.rs",
                         p.parent / decl.group(1) / "mod.rs"):
                if cand.exists():
                    _test_code[str(cand)] = True
    _test_code.setdefault(path, ranges)
    return _test_code[path]


class Table:
    """One numbering of the library lines, shared by the worker threads."""

    def __init__(self):
        self.index, self.names, self.total, self.lock = {}, [], 0, threading.Lock()

    def bits(self, lcov):
        """The bit set of the covered library lines of an lcov export (the instrumented ones join `total`)."""
        found, current = [], None
        for line in lcov.splitlines():
            if line.startswith("SF:"):
                current = line[3:]
            elif line.startswith("DA:"):
                number, count = line[3:].split(",")[:2]
                number = int(number)
                code = test_code(current)
                if code is True or number in code:
                    continue
                found.append((current, number, count != "0"))
        bits = 0
        with self.lock:
            for path, number, covered in found:
                key = (os.path.relpath(path, ROOT), number)
                if key not in self.index:
                    self.index[key] = len(self.names)
                    self.names.append(key)
                bit = 1 << self.index[key]
                self.total |= bit
                if covered:
                    bits |= bit
        return bits


def crate_of(path):
    return Path(os.path.relpath(path, ROOT)).parts[1]


def build(target_dir, rustflags):
    from rust_time import executables
    return executables(target_dir, profile="scaling", rustflags=rustflags)


def run_one(unit, work, bin_dir, extra_objects, table):
    crate, name, exe, test = unit
    key = f"{crate}/{name}::{test}"
    stem = work / f"{abs(hash(key))}"
    env = dict(os.environ, LLVM_PROFILE_FILE=f"{stem}-%p.profraw", RUST_MIN_STACK="8388608",
               OXOFFICE_RENDER="subset", CI="true")
    start = time.monotonic()
    proc = subprocess.run([exe, "--exact", test, "--test-threads", "1", "-q"], cwd=ROOT / "crates" / crate, env=env,
                          capture_output=True)
    seconds = time.monotonic() - start
    raws = list(work.glob(f"{stem.name}-*.profraw"))
    if proc.returncode or not raws:
        for r in raws:
            r.unlink()
        return key, None, seconds, proc.stdout.decode()[-500:]
    data = stem.with_suffix(".profdata")
    subprocess.run([bin_dir / "llvm-profdata", "merge", "-sparse", "-o", data, *raws], check=True)
    objects = [exe] + (extra_objects if crate == "oxoffice-cli" else [])
    cmd = [bin_dir / "llvm-cov", "export", "-format=lcov", f"-instr-profile={data}", f"-ignore-filename-regex={IGNORE}",
           "-skip-functions", objects[0]]
    for obj in objects[1:]:
        cmd += ["-object", obj]
    lcov = subprocess.run(cmd, capture_output=True, text=True, check=True).stdout
    for r in raws:
        r.unlink()
    data.unlink()
    return key, table.bits(lcov), seconds, ""


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--target-dir", type=Path, required=True, help="a target folder of its own")
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("--jobs", type=int, default=12)
    ap.add_argument("--only", help="only the binaries whose `crate/name` matches this regex")
    ap.add_argument("--skip", help="leave out the tests whose `crate/name::test` matches this regex (the render walks,"
                                   " which a cut keeps whole, cost minutes each under instrumentation)")
    args = ap.parse_args()
    target = args.target_dir.resolve()
    units = build(target, "-C instrument-coverage")
    bin_dir = tools()
    # the commands the CLI tests spawn
    extra = sorted({str(p) for p in (target / "scaling").iterdir() if p.is_file() and p.name in
                    ("oxoffice", "oxoffice-docx", "oxoffice-pptx", "oxoffice-xlsx")})
    work = Path(tempfile.mkdtemp(prefix="measure-", dir=os.environ.get("MEASURE_TMP")))
    env = dict(os.environ, LLVM_PROFILE_FILE=os.devnull, RUST_MIN_STACK="8388608")
    todo = []
    for crate, name, exe in units:
        if args.only and not re.search(args.only, f"{crate}/{name}"):
            continue
        listed = subprocess.run([exe, "--list"], cwd=ROOT / "crates" / crate, capture_output=True, text=True,
                                env=env).stdout
        todo += [(crate, name, exe, line[:-len(": test")]) for line in listed.splitlines() if line.endswith(": test")]
    if args.skip:
        todo = [u for u in todo if not re.search(args.skip, f"{u[0]}/{u[1]}::{u[3]}")]
    print(f"{len(todo)} tests; objects of the CLI: {extra}", flush=True)
    table, tests, failed = Table(), {}, []
    done = 0
    with ThreadPoolExecutor(args.jobs) as pool:
        for key, bits, seconds, tail in pool.map(lambda u: run_one(u, work, bin_dir, extra, table), todo):
            crate, name = key.split("::")[0].split("/")
            if bits is None:
                failed.append((key, tail))
            else:
                tests[key] = {"bits": bits, "s": seconds, "crate": crate, "bin": name}
            done += 1
            if done % 100 == 0:
                print(done, len(todo), len(table.names), flush=True)
    shutil.rmtree(work, ignore_errors=True)
    args.out.write_bytes(pickle.dumps({"lines": table.names, "tests": tests, "total": table.total, "failed": failed}))
    print(f"{len(tests)} tests measured, {len(failed)} failed or ignored, {len(table.names)} library lines")


if __name__ == "__main__":
    main()
