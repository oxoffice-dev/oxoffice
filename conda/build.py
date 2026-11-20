"""Install the release wheel staged in conda/dist into the conda prefix, as it is."""

import subprocess
import sys
from pathlib import Path

wheels = sorted(str(p) for p in Path.cwd().glob("*.whl"))
if len(wheels) != 1:
    sys.exit(f"expected one oxoffice wheel, found {wheels}")
subprocess.run(
    [sys.executable, "-m", "pip", "install", "--no-deps", "--no-index", "--no-build-isolation",
     "--no-compile", "--disable-pip-version-check", wheels[0]],
    check=True,
)
