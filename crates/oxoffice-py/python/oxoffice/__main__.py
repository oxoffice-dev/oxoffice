"""The ``oxoffice`` command line, as ``python -m oxoffice`` and console script.

It runs the same entry point as the native ``oxoffice`` binary, so exit
statuses, standard output and standard error are the same.
"""

from __future__ import annotations

import sys

from ._oxoffice import run_cli


def main() -> None:
    sys.stdout.flush()
    sys.stderr.flush()
    sys.exit(run_cli(["oxoffice", *sys.argv[1:]]))


if __name__ == "__main__":
    main()
