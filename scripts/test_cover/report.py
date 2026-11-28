"""Read measure.py's pickle (and rust_cpu.py's JSON): per binary, the tests, their cost and the lines only they run;
and the greedy cover that keeps a share of the library lines for the least CPU.

    python scripts/test_cover/report.py measure.pkl --cpu cpu.json [--target 0.995] [--keep REGEX ...] [--list-cut]
"""

import argparse
import collections
import json
import pickle
import re
from pathlib import Path


def popcount(x):
    return x.bit_count()


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("pickle", type=Path)
    ap.add_argument("--cpu", type=Path, required=True)
    ap.add_argument("--target", type=float, default=0.995, help="share of the lines the measured tests run to keep")
    ap.add_argument("--keep", action="append", default=[], help="regex of `crate/bin::test` kept whatever its lines")
    ap.add_argument("--only", help="regex of the binaries (`crate/bin`) the cut applies to")
    ap.add_argument("--list-cut", action="store_true")
    args = ap.parse_args()
    data = pickle.loads(args.pickle.read_bytes())
    cpu = {k: v["cpu"] for k, v in json.loads(args.cpu.read_text()).items()}
    tests = data["tests"]
    everything = 0
    for t in tests.values():
        everything |= t["bits"]
    names = data["lines"]
    keep = {k for k in tests if any(re.search(p, k) for p in args.keep)}
    if args.only:
        keep |= {k for k in tests if not re.search(args.only, f"{tests[k]['crate']}/{tests[k]['bin']}")}
    covered = 0
    for k in keep:
        covered |= tests[k]["bits"]
    goal = popcount(everything) * args.target
    # greedy: lines gained per cpu second
    chosen = set(keep)
    while popcount(covered) < goal:
        best = max((k for k in tests if k not in chosen),
                   key=lambda k: popcount(tests[k]["bits"] & ~covered) / max(cpu.get(k, 0.05), 0.02))
        chosen.add(best)
        covered |= tests[best]["bits"]
    cut = sorted(set(tests) - chosen)
    per = collections.defaultdict(lambda: [0, 0, 0.0, 0.0])
    for k in tests:
        b = f"{tests[k]['crate']}/{tests[k]['bin']}"
        per[b][0] += 1
        per[b][2] += cpu.get(k, 0)
        if k in cut:
            per[b][1] += 1
            per[b][3] += cpu.get(k, 0)
    for b, (n, c, s, cs) in sorted(per.items(), key=lambda x: -x[1][3]):
        if c:
            print(f"{b:45s} {c:5d} of {n:5d} tests cut, {cs:7.1f} of {s:7.1f} cpu s")
    print(f"total: {len(cut)} of {len(tests)} tests cut, {sum(cpu.get(k, 0) for k in cut):.0f} of "
          f"{sum(cpu.get(k, 0) for k in tests):.0f} cpu s; lines {popcount(covered)} of {popcount(everything)}")
    # per crate coverage loss
    lost = everything & ~covered
    by = collections.Counter(names[i][0].split("/")[1] for i in range(len(names)) if (lost >> i) & 1)
    allby = collections.Counter(names[i][0].split("/")[1] for i in range(len(names)) if (everything >> i) & 1)
    for crate, n in by.most_common(12):
        print(f"  lost {n:6d} lines of {allby[crate]:6d} in {crate}")
    if args.list_cut:
        for k in cut:
            print(k)


if __name__ == "__main__":
    main()
