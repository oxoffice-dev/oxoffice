"""Run every ```python and ```bash block of each skill's references/examples.md, in order, in one fresh folder per
skill that starts with the files of make_fixtures.py. Python blocks run as `$PY -c BLOCK`, bash blocks with
`eval`, both under `bash -euo pipefail`, with the variables the skills define: OX (the oxoffice command), PY (a
Python that imports oxoffice), SKILL (the skill's folder) and PYTHONPATH (its scripts). Each block's code and
output go to WORK/<skill>/_blockNN.log.

    python tests/skills/run_examples.py --skills SKILLS_DIR --fixtures DIR --work DIR [docx] [pptx]

OX defaults to the `oxoffice` on PATH, PY to this Python.
"""

import argparse
import os
import re
import shutil
import subprocess
import sys
import time
from pathlib import Path

FIXTURES = ("report.docx", "deck.pptx", "Aileron-Regular.ttf", "photo.png")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--skills", type=Path, required=True, help="the plugin's skills folder")
    ap.add_argument("--fixtures", type=Path, required=True)
    ap.add_argument("--work", type=Path, required=True)
    ap.add_argument("names", nargs="*", default=["docx", "pptx"])
    args = ap.parse_args()

    failed = 0
    for name in args.names:
        skill = (args.skills / name).resolve()
        blocks = re.findall(r"```(python|bash)\n(.*?)```", (skill / "references" / "examples.md").read_text(), re.S)
        work = args.work / name
        shutil.rmtree(work, ignore_errors=True)
        work.mkdir(parents=True)
        for f in FIXTURES:
            shutil.copy(args.fixtures / f, work / f)
        env = {k: v for k, v in os.environ.items() if k not in ("PYTHONPATH", "VIRTUAL_ENV")}
        env.setdefault("OX", "oxoffice")
        env.setdefault("PY", sys.executable)
        env.update(SKILL=skill.as_posix(), PYTHONPATH=str(skill / "scripts"), PYTHONDONTWRITEBYTECODE="1")
        for k, (lang, code) in enumerate(blocks):
            env["BLOCK"] = code
            script = '$PY -c "$BLOCK"' if lang == "python" else 'eval "$BLOCK"'
            t0 = time.monotonic()
            res = subprocess.run(["bash", "-euo", "pipefail", "-c", script], cwd=work, env=env,
                                 capture_output=True, text=True, timeout=600)
            status = "ok" if res.returncode == 0 else f"FAIL {res.returncode}"
            print(f"{name}-{k:02d}-{lang}: {status} ({time.monotonic() - t0:.1f} s)", flush=True)
            (work / f"_block{k:02d}.log").write_text(
                f"{code}\n----- stdout\n{res.stdout}\n----- stderr\n{res.stderr}")
            if res.returncode:
                failed += 1
                print("  " + (res.stdout[-1500:] + res.stderr[-2500:]).replace("\n", "\n  "))
        print(f"{name}: {len(blocks)} blocks")
    print(f"failed: {failed}")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
