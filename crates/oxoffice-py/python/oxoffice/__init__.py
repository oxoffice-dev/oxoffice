"""oxoffice: read, write, edit and render Office Open XML documents.

``oxoffice.docx`` is the Word API and ``oxoffice.pptx`` the PowerPoint API.
Import the one you need::

    from oxoffice.docx import Document
    from oxoffice.pptx import Presentation
"""

from __future__ import annotations

import json as _json
from importlib.metadata import PackageNotFoundError as _PackageNotFoundError
from importlib.metadata import version as _version

# Loading the native extension registers the ``oxoffice.docx._native`` and
# ``oxoffice.pptx._native`` modules that the two subpackages import from.
from . import _oxoffice  # noqa: F401

try:
    __version__ = _version("oxoffice")
except _PackageNotFoundError:  # pragma: no cover - source tree without metadata
    __version__ = "0.0.0"



def limitations(format: str | None = None) -> list[dict[str, object]]:
    """The known limitations of this build, with their workarounds.

    Each entry has ``id``, ``format`` (``"docx"``, ``"pptx"``, ``"xlsx"`` or
    ``"all"``), ``area``, ``title``, ``symptom``, ``workaround`` and ``issue``,
    the number of the GitHub issue that tracks it. Give ``format`` to keep only
    the limitations of one format. The command line equivalent is
    ``oxoffice limitations [docx|pptx] --json``, which gives the list under
    ``limitations``.
    """
    entries = _json.loads(_oxoffice.limitations_json())
    return [entry for entry in entries if format is None or entry["format"] in (format, "all")]


__all__ = ["__version__", "limitations"]
