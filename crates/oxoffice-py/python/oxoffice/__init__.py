"""oxoffice: read, write, edit and render Office Open XML documents.

``oxoffice.docx`` is the Word API and ``oxoffice.pptx`` the PowerPoint API.
Import the one you need::

    from oxoffice.docx import Document
    from oxoffice.pptx import Presentation
"""

from __future__ import annotations

from importlib.metadata import PackageNotFoundError as _PackageNotFoundError
from importlib.metadata import version as _version

# Loading the native extension registers the ``oxoffice.docx._native`` and
# ``oxoffice.pptx._native`` modules that the two subpackages import from.
from . import _oxoffice  # noqa: F401

try:
    __version__ = _version("oxoffice")
except _PackageNotFoundError:  # pragma: no cover - source tree without metadata
    __version__ = "0.0.0"

__all__ = ["__version__"]
