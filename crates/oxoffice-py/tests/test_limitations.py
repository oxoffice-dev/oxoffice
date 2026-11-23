"""oxoffice.limitations() gives the same list as the command line, filtered by format."""

import json
import subprocess
import sys

import oxoffice


def cli_limitations(*args):
    out = subprocess.run(
        [sys.executable, "-m", "oxoffice", "limitations", *args, "--json"],
        check=True,
        capture_output=True,
        # The command line writes UTF-8, whatever the platform's locale.
        encoding="utf-8",
    ).stdout
    return json.loads(out)


def test_limitations_match_the_command_line():
    assert oxoffice.limitations() == cli_limitations()


def test_a_format_keeps_only_its_limitations():
    for fmt in ("docx", "pptx"):
        entries = oxoffice.limitations(fmt)
        assert entries == cli_limitations(fmt)
        assert all(entry["format"] in (fmt, "all") for entry in entries)
