"""Author the corpus source documents into files/authored/, then rewrite the manifest.

    uv run --no-project --with python-docx==1.2.0 --with python-pptx==1.0.2 \
        --with pillow==13.0.0 --with XlsxWriter==3.3.3 \
        tests/corpus/generate/author.py [family ...]

Every family by default. The producers (produce.py) then open and save these
sources. Families live in docx_families.py and pptx_families.py.
"""

import sys
import time

import docx
import pptx

import docx_families  # noqa: F401  (registers the Word families)
import manifest
import pptx_families  # noqa: F401  (registers the PowerPoint families)
from common import AUTHORED, FAMILIES, normalize_zip


def main() -> None:
    names = sys.argv[1:] or sorted(FAMILIES)
    unknown = [name for name in names if name not in FAMILIES]
    if unknown:
        sys.exit(f"unknown families {unknown}, known: {sorted(FAMILIES)}")
    AUTHORED.mkdir(parents=True, exist_ok=True)
    started = time.time()
    for name in names:
        source = FAMILIES[name]
        source.build(AUTHORED / source.filename)
        normalize_zip(AUTHORED / source.filename)
        print(f"authored {source.filename}")
    manifest.update(
        {
            ("authored", "docx"): f"python-docx {docx.__version__}",
            ("authored", "pptx"): f"python-pptx {pptx.__version__}",
        },
        since=started,
    )


if __name__ == "__main__":
    main()
