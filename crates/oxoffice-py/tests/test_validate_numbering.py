"""Document.validate() and validate_bytes warn about a paragraph numbered by a numbering instance the document does
not define, and the file stays valid."""

import io
import re
import zipfile
from pathlib import Path

import oxoffice.docx as oxdocx

CONTRACT = Path(__file__).resolve().parents[3] / "tests" / "corpus" / "files" / "word" / "contract.docx"


def with_dangling_reference():
    source = zipfile.ZipFile(io.BytesIO(CONTRACT.read_bytes()))
    output = io.BytesIO()
    with zipfile.ZipFile(output, "w", zipfile.ZIP_DEFLATED) as target:
        for item in source.infolist():
            data = source.read(item.filename)
            if item.filename == "word/document.xml":
                data = re.sub(rb'<w:numId w:val="[0-9]+"/>', b'<w:numId w:val="4242"/>', data, count=1)
            target.writestr(item, data)
    return output.getvalue()


def test_validation_warns_about_a_dangling_numbering_reference():
    data = with_dangling_reference()
    for report in (oxdocx.validate_bytes(data), oxdocx.Document.from_bytes(data).validate()):
        dangling = [finding for finding in report.findings
                    if "numbering instance 4242, which the document does not define" in finding.message]
        assert len(dangling) == 1
        assert dangling[0].severity == "warning"
        assert report.is_valid
