from collections.abc import Sequence

def run_cli(argv: Sequence[str]) -> int:
    """Run the oxoffice command line on ``argv``, the program name first.

    Returns 0 when the command succeeds. A failure or a verdict ends the
    process with its exit status, as the native binary does.
    """

def limitations_json() -> str:
    """The known limitations of this build as a JSON array."""
