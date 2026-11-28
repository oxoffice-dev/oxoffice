"""Regenerate `SAMPLE` of crates/oxoffice-docx/tests/docx/block_locations.rs: the cells of the block operation grid
that a pull request runs (the full checks run all of them).

    python scripts/test_cover/grid_sample.py --target-dir ../target-grid-cov [--write]

Lists the grid's cells (`OXOFFICE_GRID_CELL=list`), runs each alone under `-C instrument-coverage`
(`OXOFFICE_GRID_CELL=<index>`), then picks cells greedily until the sample meets every shape, every operation, every
(operation, refusal message) pair of the grid and every library line the whole grid runs. Prints the `SAMPLE` rows;
with `--write` replaces them in the test file.

Needs the `llvm-tools` component of the pinned toolchain (`rustup component add llvm-tools`), Linux or macOS, and a
target folder of its own: the build is instrumented, in the `scaling` profile.
"""

import argparse
import os
import re
import subprocess
import sys
import tempfile
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from measure import IGNORE, ROOT, Table, tools  # noqa: E402

TEST = "block_locations::every_block_operation_takes_every_location_shape_or_names_the_working_route"
FILE = ROOT / "crates/oxoffice-docx/tests/docx/block_locations.rs"


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--target-dir", type=Path, required=True)
    ap.add_argument("--jobs", type=int, default=8)
    ap.add_argument("--write", action="store_true")
    args = ap.parse_args()
    env = dict(os.environ, CARGO_TARGET_DIR=str(args.target_dir.resolve()), RUSTFLAGS="-C instrument-coverage")
    built = subprocess.run(["cargo", "test", "--profile", "scaling", "--locked", "-p", "oxoffice-docx", "--test", "docx",
                            "--no-run", "--message-format=json"], env=env, cwd=ROOT, capture_output=True, text=True,
                           check=True).stdout
    import json
    exe = next(j["executable"] for j in (json.loads(l) for l in built.splitlines() if l.startswith("{"))
               if j.get("executable") and j["target"]["name"] == "docx")
    run_env = dict(os.environ, RUST_MIN_STACK="8388608", OXOFFICE_RENDER="off", LLVM_PROFILE_FILE=os.devnull)
    listed = subprocess.run([exe, TEST, "--exact", "--nocapture", "--test-threads", "1"], cwd=ROOT / "crates/oxoffice-docx",
                            env=dict(run_env, OXOFFICE_GRID_CELL="list"), capture_output=True, text=True, check=True).stdout
    cells = [line.split("\t") for line in listed.splitlines() if line.startswith("CELL\t")]
    print(len(cells), "cells", flush=True)
    bin_dir, table, work = tools(), Table(), Path(tempfile.mkdtemp(prefix="grid-", dir=os.environ.get("MEASURE_TMP")))

    def one(cell):
        raw, data = work / f"{cell[1]}.profraw", work / f"{cell[1]}.profdata"
        proc = subprocess.run([exe, TEST, "--exact", "--test-threads", "1"], cwd=ROOT / "crates/oxoffice-docx",
                              env=dict(run_env, OXOFFICE_GRID_CELL=cell[1], LLVM_PROFILE_FILE=str(raw)),
                              capture_output=True, text=True)
        assert proc.returncode == 0, (cell, proc.stdout[-500:])
        subprocess.run([bin_dir / "llvm-profdata", "merge", "-sparse", "-o", data, raw], check=True)
        lcov = subprocess.run([bin_dir / "llvm-cov", "export", "-format=lcov", f"-instr-profile={data}",
                               f"-ignore-filename-regex={IGNORE}", "-skip-functions", exe], capture_output=True,
                              text=True, check=True).stdout
        raw.unlink()
        data.unlink()
        return cell, table.bits(lcov)

    with ThreadPoolExecutor(args.jobs) as pool:
        results = list(pool.map(one, cells))
    # what each cell meets: its shape, its operation, its (operation, message) pair, its lines
    needs, lines = {}, 0
    for (_, _, op, shape, message), bits in results:
        needs[(op, shape)] = ({("shape", shape), ("op", op)} | ({("pair", op, message)} if message else set()), bits)
        lines |= bits
    wanted = set().union(*(n for n, _ in needs.values()))
    covered, got, chosen = 0, set(), []
    while got != wanted or covered != lines:
        best = max(needs, key=lambda c: 1000 * len(needs[c][0] - got) + (needs[c][1] & ~covered).bit_count())
        if not (needs[best][0] - got) and not (needs[best][1] & ~covered):
            break
        chosen.append(best)
        got |= needs[best][0]
        covered |= needs[best][1]
    order = [c for _, _, op, shape, _ in (r[0] for r in results) for c in [(op, shape)] if c in chosen]
    rows = "".join(f'    ("{op}", "{shape}"),\n' for op, shape in order)
    print(f"{len(order)} of {len(cells)} cells\n{rows}")
    if args.write:
        text = FILE.read_text()
        text = re.sub(r"(const SAMPLE: &\[\(&str, &str\)\] = &\[\n).*?(\];)", lambda m: m.group(1) + rows + m.group(2),
                      text, count=1, flags=re.S)
        FILE.write_text(text)


if __name__ == "__main__":
    main()
