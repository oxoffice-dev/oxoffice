"""Run every block of each skill's references/examples.md, in order, in one fresh folder per skill that starts
with the files of make_fixtures.py. Every block is ```python or ```bash, indented or not; any other fence
(another language, none, or ~~~) fails the run, as does a file with no block. Python blocks run as `oxpy -c BLOCK`, bash blocks with `eval`, both under
`bash -euo pipefail`, with what the skills' Setup defines: the shell functions `ox` (the oxoffice command) and
`oxpy` (a Python that imports oxoffice), SKILL (the skill's folder) and PYTHONPATH (its scripts). Each block's
code and output go to WORK/<skill>/_blockNN.log.

    python tests/skills/run_examples.py --skills SKILLS_DIR --fixtures DIR --work DIR [docx] [pptx]

The environment variables OX and PY give the commands behind `ox` and `oxpy` (default: the `oxoffice` on PATH
and this Python).
"""

import argparse
import os
import re
import shlex
import shutil
import signal
import subprocess
import sys
import textwrap
import time
from pathlib import Path

FIXTURES = ("report.docx", "deck.pptx", "Aileron-Regular.ttf", "photo.png")
TIMEOUT = 600   # seconds a block may take


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
        text = (skill / "references" / "examples.md").read_text().replace("\r\n", "\n")
        fenced = re.findall(r"^([ \t]*)```(\S*)\n(.*?)^\1```[ \t]*$", text, re.S | re.M)
        blocks = [(lang, textwrap.dedent(code)) for _, lang, code in fenced]
        others = sorted({lang or "(no language)" for lang, _ in blocks if lang not in ("python", "bash")})
        others += ["~~~"] if re.search(r"^[ \t]*~~~", text, re.M) else []
        if len(re.findall(r"^[ \t]*```", text, re.M)) != 2 * len(fenced):   # a fence closed at another indent
            others += ["unpaired fence"]
        if not blocks or others:
            print(f"{name}: {len(blocks)} blocks, fences that do not run: {others or 'none'}")
            failed += 1
            continue
        work = args.work / name
        shutil.rmtree(work, ignore_errors=True)
        work.mkdir(parents=True)
        for f in FIXTURES:
            shutil.copy(args.fixtures / f, work / f)
        env = {k: v for k, v in os.environ.items() if k not in ("PYTHONPATH", "VIRTUAL_ENV")}
        env.update(SKILL=skill.as_posix(), PYTHONPATH=str(skill / "scripts"), PYTHONDONTWRITEBYTECODE="1")
        setup = (f'ox() {{ {os.environ.get("OX", "oxoffice")} "$@"; }}; '
                 f'oxpy() {{ {os.environ.get("PY", shlex.quote(sys.executable))} "$@"; }}\n')
        for k, (lang, code) in enumerate(blocks):
            env["BLOCK"] = code
            script = setup + ('oxpy -c "$BLOCK"' if lang == "python" else 'eval "$BLOCK"')
            t0 = time.monotonic()
            # a session of its own, so that a timeout stops the block's oxoffice or Python too, not only bash
            proc = subprocess.Popen(["bash", "-euo", "pipefail", "-c", script], cwd=work, env=env, text=True,
                                    stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True)
            try:
                out, err = proc.communicate(timeout=TIMEOUT)
                status = "ok" if proc.returncode == 0 else f"FAIL {proc.returncode}"
            except subprocess.TimeoutExpired:
                os.killpg(proc.pid, signal.SIGKILL)
                out, err = proc.communicate()
                status = f"FAIL timeout after {TIMEOUT} s"
            print(f"{name}-{k:02d}-{lang}: {status} ({time.monotonic() - t0:.1f} s)", flush=True)
            (work / f"_block{k:02d}.log").write_text(f"{code}\n----- stdout\n{out}\n----- stderr\n{err}")
            if status != "ok":
                failed += 1
                print("  " + (out[-1500:] + err[-2500:]).replace("\n", "\n  "))
        print(f"{name}: {len(blocks)} blocks")
    print(f"failed: {failed}")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
