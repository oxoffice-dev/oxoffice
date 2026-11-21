"""What the corpus generators share: paths, the family registry, text and pictures.

A family is one kind of document a user has (a contract, a thesis, a pitch
deck). Its builder writes the source file into files/authored/; the producers
(produce.py) then open and save it. Builders are registered with @family and
must be deterministic: same content on every run.
"""

import io
import random
import zipfile
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path

from PIL import Image, ImageDraw

CORPUS = Path(__file__).resolve().parent.parent
FILES = CORPUS / "files"
AUTHORED = FILES / "authored"

# Dates written into the sources (core properties, comments, revisions), fixed
# so that a rerun writes the same content.
DATE = "2026-11-21T09:00:00Z"
LATER = "2026-11-21T14:30:00Z"


@dataclass(frozen=True)
class Family:
    name: str
    extension: str
    build: Callable[[Path], None]

    @property
    def filename(self) -> str:
        return f"{self.name}.{self.extension}"


FAMILIES: dict[str, Family] = {}


def family(name: str, extension: str):
    """Register a builder: `@family("contract", "docx")` over `def contract(path): ...`."""

    def register(build: Callable[[Path], None]) -> Callable[[Path], None]:
        assert name not in FAMILIES, f"family {name} registered twice"
        FAMILIES[name] = Family(name, extension, build)
        return build

    return register


CHART = Path(__file__).resolve().parent / "chart.png"
DEFAULT_CHART = ((42, 55, 61, 58, 73, 80), 900, 450, (79, 70, 229))


def chart_png(values=(42, 55, 61, 58, 73, 80), width=900, height=450, color=(79, 70, 229)) -> io.BytesIO:
    """A bar chart as a small PNG, for documents that carry a picture.

    The default chart is read from chart.png, the bytes the pilot sources
    carry: PNG compression differs between machines, and every producer's copy
    of the report and the deck holds that same picture."""
    if (values, width, height, color) == DEFAULT_CHART:
        return io.BytesIO(CHART.read_bytes())
    image = Image.new("RGB", (width, height), "white")
    draw = ImageDraw.Draw(image)
    bar = width // (len(values) * 2)
    for i, value in enumerate(values):
        x = bar // 2 + i * 2 * bar
        top = height - 40 - int(value / 100 * (height - 80))
        draw.rectangle([x, top, x + bar, height - 40], fill=color)
        draw.text((x, height - 30), f"Q{i % 4 + 1}", fill="black")
    draw.line([20, height - 40, width - 20, height - 40], fill="black", width=2)
    buffer = io.BytesIO()
    image.save(buffer, format="PNG")
    buffer.seek(0)
    return buffer


def logo_png(text="NW", size=240, color=(15, 118, 110)) -> io.BytesIO:
    """A square logo with initials, for letterheads and title slides."""
    image = Image.new("RGB", (size, size), color)
    draw = ImageDraw.Draw(image)
    draw.ellipse([size // 8, size // 8, size * 7 // 8, size * 7 // 8], outline="white", width=size // 24)
    draw.text((size // 2 - 12, size // 2 - 6), text, fill="white")
    buffer = io.BytesIO()
    image.save(buffer, format="PNG", optimize=True)
    buffer.seek(0)
    return buffer


_SUBJECTS = (
    "The operations team", "The steering committee", "Our regional partners", "The finance department",
    "The support organisation", "The new billing platform", "The southern warehouse", "Each business unit",
    "The audit working group", "The customer advisory board", "The product team", "The procurement office",
)
_VERBS = (
    "reviewed", "confirmed", "challenged", "documented", "extended", "simplified", "measured", "postponed",
    "approved", "reorganised", "monitored", "consolidated",
)
_OBJECTS = (
    "the quarterly targets", "the service level commitments", "the migration schedule", "the cost allocation model",
    "the onboarding process", "the risk register", "the supplier contracts", "the reporting calendar",
    "the capacity forecast", "the training plan", "the data retention policy", "the escalation path",
)
_TAILS = (
    "before the end of the period.", "with the help of external advisers.", "after a short consultation.",
    "in line with the decisions of the last meeting.", "so that the next review starts from shared figures.",
    "while keeping the existing budget.", "for every site, including the new ones.",
    "and the results are summarised in the appendix.", "as requested by the board in March.",
    "without changing the overall timeline.",
)


def sentences(seed: int, count: int) -> list[str]:
    """Plausible business sentences, the same for the same seed."""
    rng = random.Random(seed)
    return [
        f"{rng.choice(_SUBJECTS)} {rng.choice(_VERBS)} {rng.choice(_OBJECTS)} {rng.choice(_TAILS)}"
        for _ in range(count)
    ]


def paragraph_text(seed: int, length: int = 4) -> str:
    """One paragraph of `length` sentences."""
    return " ".join(sentences(seed, length))


def normalize_zip(path: Path) -> None:
    """Rewrite a package with fixed timestamps and order, so that a rerun on
    the same content writes the same bytes and leaves git quiet."""
    with zipfile.ZipFile(path) as package:
        parts = [(info.filename, package.read(info)) for info in package.infolist()]
    first = [p for p in parts if p[0] == "[Content_Types].xml"]
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as package:
        for name, data in first + [p for p in parts if p[0] != "[Content_Types].xml"]:
            package.writestr(zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0)), data, zipfile.ZIP_DEFLATED)
